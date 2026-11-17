// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS backend: ScreenCaptureKit system audio (macOS 13.0+).
//!
//! # API inventory (every symbol and its first macOS version)
//!
//! | API | macOS |
//! |---|---|
//! | `SCShareableContent getShareableContentWithCompletionHandler:` | 12.3 |
//! | `SCContentFilter initWithDisplay:excludingWindows:` | 12.3 |
//! | `SCStream initWithFilter:configuration:delegate:`, `addStreamOutput:type:sampleHandlerQueue:error:`, `startCapture...`, `stopCapture...` | 12.3 |
//! | `SCStreamConfiguration` width, height, minimumFrameInterval, showsCursor | 12.3 |
//! | `SCStreamConfiguration` capturesAudio, sampleRate, channelCount, excludesCurrentProcessAudio | **13.0** |
//! | `SCStreamOutputType.audio` | 13.0 (the enum is 12.3; audio output only works from 13.0) |
//! | `SCStreamDelegate stream:didStopWithError:` | 12.3 |
//! | `CGPreflightScreenCaptureAccess`, `CGRequestScreenCaptureAccess` | 10.15 |
//! | `CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer` and friends | 10.7 |
//! | Core Audio HAL properties (coreaudio.rs) | 10.x |
//!
//! Nothing newer than 13.0 is called. The 13.0 APIs sit behind a runtime
//! `available!(macos = 13.0)` check, and the binary's deployment target is
//! 13.0 anyway (scripts/check-macos-minos.sh proves it in CI).
//!
//! # Threads
//!
//! `run_session` runs on the recorder's private worker thread and owns every
//! Objective-C object. ScreenCaptureKit calls the output object on two serial
//! dispatch queues we create: one for audio (converted and handed to the
//! pipeline), one for the throw-away video frames (counted and dropped).

#![allow(unused_unsafe)] // Some objc2 bindings are safe on one SDK version and unsafe on another.

mod coreaudio;

use std::panic::AssertUnwindSafe;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMClock, CMSampleBuffer,
    CMTime, CMTimeFlags, kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
};
use objc2_foundation::{NSArray, NSError, NSProcessInfo};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamErrorCode, SCStreamErrorDomain, SCStreamOutput, SCStreamOutputType,
    SCWindow,
};

use crate::error::RecorderError;
use crate::format::{
    CaptureDevice, CaptureFormat, CaptureSetup, OutputDeviceInfo, SCK_SUPPORTED_RATES,
    SourceEncoding, choose_sck_rate,
};
use crate::pipeline::{CapturePipeline, MediaTime};
use crate::{Backend, PermissionState, RecorderConfig};

pub(crate) const BACKEND: Backend = Backend::ScreenCaptureKit;

/// The throw-away video stream sizes tried in order. ScreenCaptureKit needs a
/// display to capture even for audio; we ask for the smallest picture first.
/// If the OS rejects a size, the next one is tried and the failure is noted in
/// the report (the first run on real hardware will tell us which one works).
const VIDEO_SIZES: [(usize, usize); 3] = [(2, 2), (16, 16), (128, 72)];

/// How long to wait for ScreenCaptureKit completion handlers.
const SCK_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Permission and system information
// ---------------------------------------------------------------------------

pub(crate) fn permission_preflight() -> PermissionState {
    if CGPreflightScreenCaptureAccess() {
        PermissionState::Granted
    } else {
        PermissionState::NotGranted
    }
}

pub(crate) fn permission_request() -> PermissionState {
    if CGRequestScreenCaptureAccess() {
        PermissionState::Granted
    } else {
        PermissionState::NotGranted
    }
}

pub(crate) fn output_device() -> Option<OutputDeviceInfo> {
    coreaudio::default_output_device()
}

/// ScreenCaptureKit records the system mix, not a chosen device, so there is
/// nothing to choose on macOS.
pub(crate) fn capture_devices() -> Result<Vec<CaptureDevice>, RecorderError> {
    Ok(Vec::new())
}

/// "Now" on the host-time clock (`CMClockGetHostTimeClock`, macOS 10.7+), the
/// clock ScreenCaptureKit stamps its sample buffers with. Used for the
/// recording's start and stop instants; the pipeline refuses (and counts) an
/// extension if the buffers turn out to use a different clock.
fn host_now() -> Option<MediaTime> {
    // SAFETY: CoreMedia's singleton host clock; CMClockGetTime has no preconditions.
    let t: CMTime = unsafe { CMClock::host_time_clock().time() };
    (t.flags.contains(CMTimeFlags::Valid) && t.timescale > 0).then_some(MediaTime {
        value: t.value,
        timescale: t.timescale as u32,
    })
}

pub(crate) fn os_version() -> String {
    let info = NSProcessInfo::processInfo();
    format!("macOS {}", info.operatingSystemVersionString())
}

// ---------------------------------------------------------------------------
// Errors from NSError
// ---------------------------------------------------------------------------

/// The parts of an `NSError` we keep (plain Rust data, so it can cross threads).
#[derive(Clone, Debug)]
struct ErrInfo {
    domain: String,
    code: i64,
    description: String,
}

impl ErrInfo {
    fn from_ns(e: &NSError) -> Self {
        Self {
            domain: e.domain().to_string(),
            code: e.code() as i64,
            description: e.localizedDescription().to_string(),
        }
    }

    fn internal(msg: &str) -> Self {
        Self {
            domain: "jk-recorder".into(),
            code: -1,
            description: msg.into(),
        }
    }

    /// `SCStreamErrorUserDeclined` (-3801): the Screen Recording permission is missing.
    fn is_permission(&self) -> bool {
        // SAFETY: reading an immutable framework constant.
        let sck_domain = unsafe { SCStreamErrorDomain }.to_string();
        self.code == SCStreamErrorCode::UserDeclined.0 as i64
            && (self.domain == sck_domain || self.domain.contains("ScreenCaptureKit"))
    }

    fn into_error(self, doing: &str) -> RecorderError {
        if self.is_permission() {
            return RecorderError::PermissionDenied(format!(
                "macOS refused screen capture while {doing} ({}). Turn Jukeoke on in System Settings > \
                 Privacy & Security > Screen Recording, then quit and reopen Jukeoke.",
                self.description
            ));
        }
        RecorderError::Backend {
            domain: self.domain,
            code: self.code,
            message: format!("{doing}: {}", self.description),
        }
    }
}

/// Wrapper that lets an Objective-C object cross from a ScreenCaptureKit
/// completion queue to our worker thread.
///
/// SAFETY: only used for `SCShareableContent`, an immutable snapshot that is
/// never mutated after the completion handler hands it over; ownership moves
/// to exactly one thread.
struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

// ---------------------------------------------------------------------------
// The SCStreamOutput object (receives audio and video sample buffers)
// ---------------------------------------------------------------------------

/// Scratch space reused across callbacks.
#[derive(Default)]
struct AudioScratch {
    interleaved: Vec<f32>,
    /// 8-byte aligned storage for an `AudioBufferList` of any channel count.
    abl: Vec<u64>,
}

struct OutputIvars {
    pipeline: Arc<CapturePipeline>,
    scratch: Mutex<AudioScratch>,
    screen_frames: AtomicU64,
    audio_buffers: AtomicU64,
    audio_errors: AtomicU64,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - StreamOutput does not implement Drop (its ivars do, which is allowed).
    #[unsafe(super(NSObject))]
    #[name = "JukeokeRecorderStreamOutput"]
    #[ivars = OutputIvars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            // Never unwind into Objective-C.
            let r = std::panic::catch_unwind(AssertUnwindSafe(|| self.handle(sample_buffer, kind)));
            if r.is_err() {
                self.ivars()
                    .pipeline
                    .fail("internal error: the audio callback panicked");
            }
        }
    }
);

impl StreamOutput {
    fn new(pipeline: Arc<CapturePipeline>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            pipeline,
            scratch: Mutex::new(AudioScratch::default()),
            screen_frames: AtomicU64::new(0),
            audio_buffers: AtomicU64::new(0),
            audio_errors: AtomicU64::new(0),
        });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    fn handle(&self, sample_buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
        let iv = self.ivars();
        if kind == SCStreamOutputType::Screen {
            // The video exists only because ScreenCaptureKit requires a display.
            // Frames are counted (for the report) and discarded unseen.
            iv.screen_frames.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if kind != SCStreamOutputType::Audio {
            return;
        }
        iv.audio_buffers.fetch_add(1, Ordering::Relaxed);
        let mut scratch = iv.scratch.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: `sample_buffer` is valid for the duration of this callback.
        match unsafe { read_audio(sample_buffer, &mut scratch) } {
            Ok(Some((format, start))) => {
                if iv.pipeline.set_format(format).is_ok() {
                    let _ = iv.pipeline.push(start, &scratch.interleaved);
                }
                // A changed format stops the recording: the pipeline closes
                // the file and reports it (state Failed with a message).
            }
            Ok(None) => {}
            Err(msg) => {
                // Refuse to write data we cannot interpret; say exactly why.
                if iv.audio_errors.fetch_add(1, Ordering::Relaxed) == 0 {
                    iv.pipeline.fail(msg);
                }
            }
        }
    }
}

/// Convert one audio `CMSampleBuffer` to interleaved f32 in `scratch`.
/// Returns the real format and the timestamp of the first frame.
///
/// # Safety
/// `sb` must be a valid sample buffer for the duration of the call.
unsafe fn read_audio(
    sb: &CMSampleBuffer,
    scratch: &mut AudioScratch,
) -> Result<Option<(CaptureFormat, Option<MediaTime>)>, String> {
    unsafe {
        if !sb.is_valid() || !sb.data_is_ready() {
            return Ok(None);
        }
        let frames = sb.num_samples();
        if frames <= 0 {
            return Ok(None);
        }
        let frames = frames as usize;
        let desc = sb
            .format_description()
            .ok_or("an audio buffer arrived without a format description")?;
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&desc)
            .as_ref()
            .copied()
            .ok_or("the audio format description has no AudioStreamBasicDescription")?;

        if asbd.mFormatID != kAudioFormatLinearPCM {
            return Err(format!(
                "macOS delivered non-PCM audio (format id {:#010x})",
                asbd.mFormatID
            ));
        }
        let flags = asbd.mFormatFlags;
        if flags & kAudioFormatFlagIsBigEndian != 0 {
            return Err(
                "macOS delivered big-endian audio, which this recorder does not handle".into(),
            );
        }
        let channels = asbd.mChannelsPerFrame as usize;
        if channels == 0 || channels > u16::MAX as usize {
            return Err(format!("macOS delivered audio with {channels} channels"));
        }
        let interleaved = flags & kAudioFormatFlagIsNonInterleaved == 0;
        let source = match (
            flags & kAudioFormatFlagIsFloat != 0,
            flags & kAudioFormatFlagIsSignedInteger != 0,
            asbd.mBitsPerChannel,
        ) {
            (true, _, 32) => SourceEncoding::Float32 { interleaved },
            (false, true, 16) => SourceEncoding::Int16 { interleaved },
            (false, true, 32) => SourceEncoding::Int32 { interleaved },
            (f, s, b) => {
                return Err(format!(
                    "macOS delivered PCM we do not handle (float={f}, signed={s}, {b} bits)"
                ));
            }
        };
        let rate = asbd.mSampleRate;
        if !(rate > 0.0 && rate.fract() == 0.0 && rate <= f64::from(u32::MAX)) {
            return Err(format!(
                "macOS delivered audio at a non-integer rate ({rate} Hz)"
            ));
        }

        // Size query, then the real call into 8-byte aligned storage.
        let mut needed: usize = 0;
        let status = sb.audio_buffer_list_with_retained_block_buffer(
            &mut needed,
            null_mut(),
            0,
            None,
            None,
            0,
            null_mut(),
        );
        if status != 0 || needed < size_of::<AudioBufferList>() {
            return Err(format!(
                "CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer (size query) failed: OSStatus {status}"
            ));
        }
        let words = needed.div_ceil(8);
        if scratch.abl.len() < words {
            scratch.abl.resize(words, 0);
        }
        let abl = scratch.abl.as_mut_ptr().cast::<AudioBufferList>();
        let mut block: *mut CMBlockBuffer = null_mut();
        let status = sb.audio_buffer_list_with_retained_block_buffer(
            null_mut(),
            abl,
            needed,
            None,
            None,
            kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
            &mut block,
        );
        // The block buffer owns the memory the AudioBufferList points into;
        // keep it alive until we have copied the samples, then release it.
        let _block_guard = NonNull::new(block).map(|b| CFRetained::from_raw(b));
        if status != 0 {
            return Err(format!(
                "CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer failed: OSStatus {status}"
            ));
        }
        let n_buffers = (*abl).mNumberBuffers as usize;
        let buffers: &[AudioBuffer] = std::slice::from_raw_parts(
            (&raw const (*abl).mBuffers).cast::<AudioBuffer>(),
            n_buffers,
        );
        let bytes_per_sample = (asbd.mBitsPerChannel / 8) as usize;
        let expected_buffers = if interleaved { 1 } else { channels };
        if n_buffers != expected_buffers {
            return Err(format!(
                "expected {expected_buffers} audio buffer(s) for {channels} channel(s), got {n_buffers}"
            ));
        }
        // Whole frames actually present in every buffer.
        let samples_per_buffer_frame = if interleaved { channels } else { 1 };
        let mut usable = frames;
        for b in buffers {
            if b.mData.is_null() {
                return Err("an audio buffer had no data pointer".into());
            }
            usable = usable
                .min(b.mDataByteSize as usize / (bytes_per_sample * samples_per_buffer_frame));
        }
        let out = &mut scratch.interleaved;
        out.clear();
        out.resize(usable * channels, 0.0);
        for (index, b) in buffers.iter().enumerate() {
            let count = usable * samples_per_buffer_frame;
            // Planar: buffer `index` is channel `index`, written with stride
            // `channels`. Interleaved: one buffer, copied straight through.
            let (offset, stride) = if interleaved {
                (0, 1)
            } else {
                (index, channels)
            };
            match source {
                SourceEncoding::Float32 { .. } => {
                    let src = std::slice::from_raw_parts(b.mData.cast::<f32>(), count);
                    for (i, &s) in src.iter().enumerate() {
                        out[offset + i * stride] = s;
                    }
                }
                SourceEncoding::Int16 { .. } => {
                    let src = std::slice::from_raw_parts(b.mData.cast::<i16>(), count);
                    for (i, &s) in src.iter().enumerate() {
                        out[offset + i * stride] = f32::from(s) / 32_768.0;
                    }
                }
                SourceEncoding::Int32 { .. } => {
                    let src = std::slice::from_raw_parts(b.mData.cast::<i32>(), count);
                    for (i, &s) in src.iter().enumerate() {
                        out[offset + i * stride] = s as f32 / 2_147_483_648.0;
                    }
                }
            }
        }

        let pts: CMTime = sb.presentation_time_stamp();
        let start = if pts.flags.contains(CMTimeFlags::Valid) && pts.timescale > 0 {
            Some(MediaTime {
                value: pts.value,
                timescale: pts.timescale as u32,
            })
        } else {
            None
        };
        Ok(Some((
            CaptureFormat {
                sample_rate: rate as u32,
                channels: channels as u16,
                source,
            },
            start,
        )))
    }
}

// ---------------------------------------------------------------------------
// The SCStreamDelegate object (hears about the stream stopping on its own)
// ---------------------------------------------------------------------------

struct DelegateIvars {
    pipeline: Arc<CapturePipeline>,
}

define_class!(
    // SAFETY: as for StreamOutput.
    #[unsafe(super(NSObject))]
    #[name = "JukeokeRecorderStreamDelegate"]
    #[ivars = DelegateIvars]
    struct StreamDelegate;

    unsafe impl NSObjectProtocol for StreamDelegate {}

    unsafe impl SCStreamDelegate for StreamDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let info = ErrInfo::from_ns(error);
            self.ivars().pipeline.fail(format!(
                "macOS stopped the capture: {} (domain {}, code {}). If the Screen Recording permission \
                 was switched off, switch it on again and reopen Jukeoke.",
                info.description, info.domain, info.code
            ));
        }
    }
);

impl StreamDelegate {
    fn new(pipeline: Arc<CapturePipeline>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { pipeline });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Everything that must stay alive while capturing. ScreenCaptureKit keeps
/// only weak references to outputs (developer reports for macOS 14), so we
/// hold strong ones here.
struct Session {
    stream: Retained<SCStream>,
    output: Retained<StreamOutput>,
    _delegate: Retained<StreamDelegate>,
    audio_queue: DispatchRetained<DispatchQueue>,
    video_queue: DispatchRetained<DispatchQueue>,
}

impl Session {
    /// Stop the stream and return notes for the report.
    fn stop(self) -> Result<Vec<String>, RecorderError> {
        let result = wait_for_completion(SCK_TIMEOUT, |block| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(block))
        });
        // Detach the output, then wait until any callback already queued on our
        // two serial queues has finished, so nothing touches the output object
        // or the pipeline after this function returns and they are released.
        let out_ref = ProtocolObject::from_ref(&*self.output);
        unsafe {
            let _ = self
                .stream
                .removeStreamOutput_type_error(out_ref, SCStreamOutputType::Audio);
            let _ = self
                .stream
                .removeStreamOutput_type_error(out_ref, SCStreamOutputType::Screen);
        }
        self.audio_queue.exec_sync(|| {});
        self.video_queue.exec_sync(|| {});
        let iv = self.output.ivars();
        let mut notes = vec![format!(
            "ScreenCaptureKit delivered {} audio buffers and {} throw-away video frames (discarded unseen)",
            iv.audio_buffers.load(Ordering::Relaxed),
            iv.screen_frames.load(Ordering::Relaxed)
        )];
        let errors = iv.audio_errors.load(Ordering::Relaxed);
        if errors > 0 {
            notes.push(format!(
                "{errors} audio buffers could not be interpreted (see error)"
            ));
        }
        match result {
            Ok(()) => Ok(notes),
            Err(e) => {
                notes.push(format!("stopCapture reported: {}", e.description));
                Ok(notes)
            }
        }
    }
}

/// Call `start` with a completion block taking an `NSError*`, and wait for it.
fn wait_for_completion(
    timeout: Duration,
    start: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), ErrInfo> {
    let (tx, rx) = mpsc::sync_channel::<Result<(), ErrInfo>>(1);
    let block = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: ScreenCaptureKit passes a valid NSError or nil.
        let r = match unsafe { error.as_ref() } {
            Some(e) => Err(ErrInfo::from_ns(e)),
            None => Ok(()),
        };
        let _ = tx.try_send(r);
    });
    start(&block);
    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| Err(ErrInfo::internal("ScreenCaptureKit did not answer in time")))
}

fn fetch_shareable_content() -> Result<Retained<SCShareableContent>, ErrInfo> {
    let (tx, rx) = mpsc::sync_channel::<Result<SendBox<Retained<SCShareableContent>>, ErrInfo>>(1);
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: ScreenCaptureKit passes valid objects or nil; we retain the
            // content because the block does not own it.
            let r = match unsafe { error.as_ref() } {
                Some(e) => Err(ErrInfo::from_ns(e)),
                None => match unsafe { Retained::retain(content) } {
                    Some(c) => Ok(SendBox(c)),
                    None => Err(ErrInfo::internal(
                        "ScreenCaptureKit returned neither content nor an error",
                    )),
                },
            };
            let _ = tx.try_send(r);
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    match rx.recv_timeout(SCK_TIMEOUT) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(ErrInfo::internal(
            "ScreenCaptureKit did not list the displays in time",
        )),
    }
}

/// The main display if ScreenCaptureKit lists it, else the first display.
fn pick_display(content: &SCShareableContent) -> Option<Retained<SCDisplay>> {
    let displays: Retained<NSArray<SCDisplay>> = unsafe { content.displays() };
    let main = CGMainDisplayID();
    let mut first = None;
    for d in displays.iter() {
        if unsafe { d.displayID() } == main {
            return Some(d);
        }
        if first.is_none() {
            first = Some(d);
        }
    }
    first
}

fn make_configuration(
    rate: u32,
    channels: u16,
    exclude_own: bool,
    width: usize,
    height: usize,
) -> Retained<SCStreamConfiguration> {
    unsafe {
        let cfg = SCStreamConfiguration::new();
        // Audio (macOS 13.0+, guarded by the available! check in start_stream).
        cfg.setCapturesAudio(true);
        cfg.setSampleRate(rate as isize);
        cfg.setChannelCount(channels as isize);
        cfg.setExcludesCurrentProcessAudio(exclude_own);
        // The smallest, slowest video ScreenCaptureKit will run (12.3+).
        cfg.setWidth(width);
        cfg.setHeight(height);
        cfg.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: 1,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        });
        cfg.setShowsCursor(false);
        cfg
    }
}

fn start_stream(
    config: &RecorderConfig,
    pipeline: &Arc<CapturePipeline>,
    requested: Instant,
) -> Result<(Session, CaptureSetup), RecorderError> {
    // Availability: SCStreamConfiguration.capturesAudio & co. arrived in 13.0.
    if !objc2::available!(macos = 13.0) {
        return Err(RecorderError::Unsupported(format!(
            "system audio capture needs macOS 13.0 or newer; this Mac runs {}",
            os_version()
        )));
    }

    // Permission. Never show the system prompt unless the caller asked for it
    // (the UI explains "Screen Recording" first).
    let mut notes = Vec::new();
    if !CGPreflightScreenCaptureAccess() {
        if !config.request_permission {
            return Err(RecorderError::PermissionDenied(
                "Screen Recording is not allowed for this app yet (CGPreflightScreenCaptureAccess returned false). \
                 Allow it in System Settings > Privacy & Security > Screen Recording, then quit and reopen."
                    .into(),
            ));
        }
        let granted = CGRequestScreenCaptureAccess();
        notes.push(format!("CGRequestScreenCaptureAccess returned {granted}"));
        // Carry on either way: ScreenCaptureKit's own answer below is definitive.
    }

    // Command-line tools must touch CoreGraphics before ScreenCaptureKit, or
    // the WindowServer connection may not be initialised.
    let _ = CGMainDisplayID();

    let device = coreaudio::default_output_device();
    let channels = config.channels.clamp(1, 2);
    if channels != config.channels {
        notes.push(format!(
            "ScreenCaptureKit offers 1 or 2 channels; asked for {channels} instead of {}",
            config.channels
        ));
    }
    let (rate, rate_note) = match config.sample_rate {
        Some(r) if SCK_SUPPORTED_RATES.contains(&r) => (r, None),
        Some(r) => (
            48_000,
            Some(format!(
                "{r} Hz is not offered by ScreenCaptureKit; asked for 48000 Hz"
            )),
        ),
        None => choose_sck_rate(device.as_ref().map(|d| d.nominal_sample_rate)),
    };
    notes.extend(rate_note);

    let content =
        fetch_shareable_content().map_err(|e| e.into_error("listing the displays to capture"))?;
    let display = pick_display(&content).ok_or_else(|| {
        RecorderError::Unsupported(
            "ScreenCaptureKit lists no display; it needs one even for audio".into(),
        )
    })?;
    let no_windows: Retained<NSArray<SCWindow>> = NSArray::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &no_windows,
        )
    };

    let output = StreamOutput::new(Arc::clone(pipeline));
    let delegate = StreamDelegate::new(Arc::clone(pipeline));
    let audio_queue = DispatchQueue::new("org.jukeoke.recorder.audio", DispatchQueueAttr::SERIAL);
    let video_queue = DispatchQueue::new("org.jukeoke.recorder.video", DispatchQueueAttr::SERIAL);

    let mut last_error = None;
    for (width, height) in VIDEO_SIZES {
        let cfg = make_configuration(rate, channels, config.exclude_own_audio, width, height);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &cfg,
                Some(ProtocolObject::from_ref(&*delegate)),
            )
        };
        let out_ref = ProtocolObject::from_ref(&*output);
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    out_ref,
                    SCStreamOutputType::Screen,
                    Some(&video_queue),
                )
                .map_err(|e| ErrInfo::from_ns(&e).into_error("adding the video output"))?;
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    out_ref,
                    SCStreamOutputType::Audio,
                    Some(&audio_queue),
                )
                .map_err(|e| ErrInfo::from_ns(&e).into_error("adding the audio output"))?;
        }
        // Start: the recording covers the time from the Recorder::start()
        // call, on the host-time clock (ScreenCaptureKit's set-up time before
        // the first buffer is filled with silence).
        if let Some(t) = host_now() {
            pipeline.mark_start_since(t.earlier_by(requested.elapsed()), requested);
        }
        match wait_for_completion(SCK_TIMEOUT, |block| unsafe {
            stream.startCaptureWithCompletionHandler(Some(block))
        }) {
            Ok(()) => {
                notes.push(format!(
                    "throw-away video stream: {width}x{height} px, at most 1 frame per second, frames discarded unseen"
                ));
                let setup = CaptureSetup {
                    backend: BACKEND.name(),
                    device,
                    requested_sample_rate: Some(rate),
                    requested_channels: channels,
                    notes,
                };
                let session = Session {
                    stream,
                    output,
                    _delegate: delegate,
                    audio_queue,
                    video_queue,
                };
                return Ok((session, setup));
            }
            Err(e) if e.is_permission() => return Err(e.into_error("starting the capture")),
            Err(e) => {
                last_error = Some(e.clone());
                // Make sure a half-started stream is not left behind.
                let _ = wait_for_completion(Duration::from_secs(2), |block| unsafe {
                    stream.stopCaptureWithCompletionHandler(Some(block))
                });
            }
        }
    }
    let e = last_error.unwrap_or_else(|| ErrInfo::internal("no video size was accepted"));
    Err(e.into_error(&format!(
        "starting the capture (tried video sizes {})",
        VIDEO_SIZES
            .iter()
            .map(|(w, h)| format!("{w}x{h}"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

pub(crate) fn run_session(
    config: &RecorderConfig,
    pipeline: Arc<CapturePipeline>,
    ready: SyncSender<Result<CaptureSetup, RecorderError>>,
    stop: Receiver<Instant>,
    requested: Instant,
) -> Result<Vec<String>, RecorderError> {
    let started = autoreleasepool(|_| start_stream(config, &pipeline, requested));
    let session = match started {
        Ok((session, setup)) => {
            let _ = ready.send(Ok(setup));
            session
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(Vec::new());
        }
    };
    // Either a stop message (the instant stop() was called) or the Recorder
    // being dropped ends the capture.
    let stop_called = stop.recv().unwrap_or_else(|_| Instant::now());
    // Stop: the recording covers the time up to that instant, even if macOS
    // sent nothing during a silent end (the pipeline fills it at finish()).
    if let Some(t) = host_now() {
        pipeline.mark_stop(t.earlier_by(stop_called.elapsed()));
    }
    autoreleasepool(|_| session.stop())
}
