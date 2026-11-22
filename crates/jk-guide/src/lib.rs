// SPDX-License-Identifier: GPL-3.0-or-later
//! # jk-guide
//!
//! Makes a song's melody guide from what the song file already holds and adds it to the
//! file (Work Order 2, section 4.3: "the guide is added from the lead vocal and lyrics
//! already stored in the file, without separating the song again"). One code path for
//! every caller, so a new song's guide is exactly what "Add practice data" makes:
//!
//! - `jukeoke-cli guide add` (crates/jk-cli) and the Studio's `make --guide` through it;
//! - the app core's guide queue (app/src-tauri/src/guide.rs): new songs after the
//!   Studio's `job.done` (docs/STUDIO_PROTOCOL.md section 12), "Add practice data",
//!   and the Mac, which has no Studio.
//!
//! Steps (docs/SONG_FORMAT.md section 7.1):
//!
//! 1. [`build_for_song`]: reads the `lead_vocal` stem (CRC-32 checked, SHA-256 taken for
//!    the provenance), decodes it with its stored gain, averages the channels, runs the
//!    shared builder `jk_pitch::guide`, and turns the result into the file's guide
//!    ([`doc_from_guide`]): notes rounded to 1 ms / 0.1 cent, spoken spans, the tuning
//!    with the 0.3 strength rule, counts, the curve (MIDI, VOIC, LVDB every 10 ms) and
//!    word references by the overlap rule of section 10.4 against the effective lyrics.
//! 2. [`prepare_write`] / [`finish_write`]: the raw-copy rewrite, split so the app can
//!    defer the swap while the song plays; [`add_to_song`] does everything in one go.
//!
//! Errors carry the codes of STUDIO_PROTOCOL 12.2 (`guide_no_lead`, `guide_read_failed`,
//! `guide_damaged`, `guide_disk_full`, `guide_read_only`, `guide_in_use`,
//! `guide_internal`, `cancelled`) and a plain sentence for the singer.
//!
//! Lyrics are read only to attach words to notes; nothing here prints them.

pub mod singable;

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use jk_format::guide::{
    CURVE_CHUNK, Curve, GUIDE_VERSION, GuideApp, GuideDoc, GuideFileState, GuideNote, GuideProvenance, GuideSource,
    Span, Tuning, WordRef, WordsRef, round_to, round3,
};
use jk_format::guide_write::{GuideChunks, PreparedGuide, RewriteControl, hex, is_cancelled};
use jk_format::{ErrorKind, FormatError, Lyrics, SongFile};
use jk_pitch::guide::words::{WordSpan, map_words};
use jk_pitch::guide::{GuideConfig, NoteKind, OctaveCheck};
use serde_json::{Map, Value};
use sha2::Digest;

/// The builder's method and version, as stored in `provenance` (SONG_FORMAT 10.2).
pub use jk_pitch::guide::{METHOD, VERSION};

/// Unpitched notes closer than this (s) belong to one spoken span (the builder's phrase gap).
pub const SPAN_GAP_S: f64 = 0.3;

/// Why a guide could not be made or written: a code from STUDIO_PROTOCOL 12.2, a plain
/// sentence for the singer and the technical detail for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct GuideError {
    pub code: &'static str,
    pub message: String,
    pub detail: String,
}

impl std::fmt::Display for GuideError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.message, self.code, self.detail)
    }
}

impl std::error::Error for GuideError {}

impl GuideError {
    fn new(code: &'static str, message: impl Into<String>, detail: impl Into<String>) -> GuideError {
        GuideError { code, message: message.into(), detail: detail.into() }
    }

    /// `guide_no_lead`.
    pub fn no_lead() -> GuideError {
        GuideError::new(
            "guide_no_lead",
            "This song has no separate lead vocal, so there is no melody to practise.",
            "no lead_vocal stem in the manifest",
        )
    }

    /// `cancelled`.
    pub fn cancelled() -> GuideError {
        GuideError::new("cancelled", "Cancelled.", "cancelled")
    }

    /// `guide_internal`.
    pub fn internal(detail: impl Into<String>) -> GuideError {
        GuideError::new(
            "guide_internal",
            "Practice data could not be made because of an internal error. Press Try again; the details are in the log.",
            detail,
        )
    }

    /// `guide_damaged`.
    pub fn damaged(detail: impl Into<String>) -> GuideError {
        GuideError::new(
            "guide_damaged",
            "Part of this song file is damaged, so practice data could not be added. The song was left as it was. Copy it again from the original, or make it again in the Studio.",
            detail,
        )
    }
}

/// Raw OS error codes that mean "no space" / "write-protected" / "in use".
fn os_code(e: &std::io::Error) -> Option<i32> {
    e.raw_os_error()
}

/// Maps an error from reading the song (step 1) to a guide error.
pub fn read_error(e: &FormatError) -> GuideError {
    match e.kind() {
        ErrorKind::Damaged | ErrorKind::MissingChunk | ErrorKind::Truncated => GuideError::damaged(e.to_string()),
        _ if is_cancelled(e) => GuideError::cancelled(),
        _ => GuideError::new(
            "guide_read_failed",
            format!("The song file could not be read, so practice data could not be made. {}", e.user_message()),
            e.to_string(),
        ),
    }
}

/// Where a write error happened (the same OS error means different things).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStage {
    /// Writing the temporary file next to the song.
    Prepare,
    /// The checks and the rename over the original.
    Swap,
}

/// Maps an error from the raw-copy rewrite to a guide error. `needed_bytes`: the size of
/// the new file, for the "free N MB" advice.
pub fn write_error(e: &FormatError, stage: WriteStage, needed_bytes: u64) -> GuideError {
    if is_cancelled(e) {
        return GuideError::cancelled();
    }
    let mb = needed_bytes.div_ceil(1024 * 1024).max(1) + 1;
    let full = || {
        GuideError::new(
            "guide_disk_full",
            format!("Practice data could not be saved: the drive is full. Free {mb} MB and press Try again."),
            e.to_string(),
        )
    };
    let read_only = || {
        GuideError::new(
            "guide_read_only",
            "Practice data could not be saved: the drive or the song file cannot be written (it may be read-only or write-protected). It is used for this session only.",
            e.to_string(),
        )
    };
    let in_use = || {
        GuideError::new(
            "guide_in_use",
            "Practice data could not be saved: another program has the song file open. Close it and press Try again.",
            e.to_string(),
        )
    };
    match e {
        FormatError::Io(io) => {
            use std::io::ErrorKind as K;
            // The raw numbers are Windows error codes or Unix errnos, each only on its own
            // system (verifier: on macOS 19, 32, 33 and 39 are ENODEV, EPIPE, EDOM and
            // EDESTADDRREQ, which must not read as "write-protected" or "in use").
            match (io.kind(), os_code(io)) {
                (K::StorageFull, _) => full(),
                // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL.
                #[cfg(windows)]
                (_, Some(112 | 39)) => full(),
                // ENOSPC.
                #[cfg(unix)]
                (_, Some(28)) => full(),
                (K::ReadOnlyFilesystem, _) => read_only(),
                // ERROR_WRITE_PROTECT.
                #[cfg(windows)]
                (_, Some(19)) => read_only(),
                // EROFS.
                #[cfg(unix)]
                (_, Some(30)) => read_only(),
                // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
                #[cfg(windows)]
                (_, Some(32 | 33)) => in_use(),
                // Access denied when the temporary file is created: the folder is not
                // writable. At the swap (the folder was writable a moment ago): the file
                // is held open by another program.
                (K::PermissionDenied, _) if stage == WriteStage::Prepare => read_only(),
                (K::PermissionDenied, _) => in_use(),
                _ => GuideError::internal(e.to_string()),
            }
        }
        _ => match e.kind() {
            ErrorKind::Damaged | ErrorKind::MissingChunk | ErrorKind::Truncated => GuideError::damaged(e.to_string()),
            ErrorKind::ChangedOnDisk => GuideError::new(
                "guide_internal",
                "The song file changed while its practice data was being made. Press Try again.",
                e.to_string(),
            ),
            _ => GuideError::internal(e.to_string()),
        },
    }
}

/// Who made the guide (`provenance.app`).
pub fn app_stamp(name: &str, version: &str) -> GuideApp {
    GuideApp {
        name: name.into(),
        version: version.into(),
        // "windows" or "macos" (SONG_FORMAT 10.2); other systems report their own name.
        platform: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        extra: Map::new(),
    }
}

/// Cancellation and progress (0..1 of the build) for [`build_for_song`].
#[derive(Default)]
pub struct BuildControl<'a> {
    pub cancel: Option<&'a AtomicBool>,
    pub progress: Option<&'a mut dyn FnMut(f64)>,
}

impl BuildControl<'_> {
    fn check(&self) -> Result<(), GuideError> {
        if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(GuideError::cancelled()) } else { Ok(()) }
    }
    fn report(&mut self, p: f64) {
        if let Some(f) = self.progress.as_mut() {
            f(p.clamp(0.0, 1.0));
        }
    }
}

/// A guide made for a song, ready to be written.
#[derive(Clone, Debug)]
pub struct Built {
    pub doc: GuideDoc,
    pub curve: Curve,
    /// Build time (s), also in `provenance.seconds`.
    pub seconds: f64,
}

impl Built {
    /// The two chunks as stored.
    pub fn chunks(&self) -> GuideChunks {
        GuideChunks { guide_json: self.doc.to_json(), curve: self.curve.encode() }
    }
}

/// What the conversion needs besides the builder's output.
pub struct DocContext<'a> {
    pub duration_s: f64,
    pub cfg: &'a GuideConfig,
    pub source: GuideSource,
    pub analysis_rate_hz: f64,
    pub app: GuideApp,
    pub created_utc: String,
    pub seconds: f64,
}

fn octave_str(o: OctaveCheck) -> &'static str {
    match o {
        OctaveCheck::Ok => "ok",
        OctaveCheck::MaybeHigh => "maybe_high",
        OctaveCheck::MaybeLow => "maybe_low",
        OctaveCheck::Unchecked => "unchecked",
    }
}

/// Turns the builder's guide into the file's guide and curve (pure; no word references
/// yet, see [`set_word_refs`]).
pub fn doc_from_guide(g: &jk_pitch::guide::Guide, ctx: &DocContext<'_>) -> (GuideDoc, Curve) {
    let tmax = ctx.duration_s;
    let mut notes: Vec<GuideNote> = Vec::with_capacity(g.notes.len());
    for n in &g.notes {
        let start = round3(n.start_s.max(0.0));
        let end = round3(n.end_s.min(tmax));
        if !(end > start) {
            continue;
        }
        // Rounding never makes neighbours overlap by more than a rounding step; keep them
        // strictly ordered anyway.
        let start = notes.last().map_or(start, |p| if start < p.end { p.end } else { start });
        if !(end > start) {
            continue;
        }
        let pitched = n.kind == NoteKind::Pitched;
        let midi = if n.midi.is_finite() && n.midi > 0.0 { round3(n.midi.clamp(12.0, 120.0)) } else { 0.0 };
        let midi = if pitched && midi == 0.0 { continue } else { midi };
        notes.push(GuideNote {
            start,
            end,
            midi,
            confidence: round3((n.confidence as f64).clamp(0.0, 1.0)),
            kind: if pitched { "pitched".into() } else { "unpitched".into() },
            octave: octave_str(n.octave).into(),
            glide_in: round3(n.glide_in_s.max(0.0)).min(round3(end - start)),
            word: None,
            extra: Map::new(),
        });
    }
    // Spoken spans: runs of unpitched notes with gaps under the phrase gap.
    let mut spans: Vec<Span> = Vec::new();
    let mut run: Option<(f64, f64)> = None;
    for n in &notes {
        if n.is_pitched() {
            if let Some((a, b)) = run.take() {
                spans.push(Span { start: a, end: b, kind: "spoken".into(), extra: Map::new() });
            }
            continue;
        }
        run = match run {
            Some((a, b)) if n.start - b < SPAN_GAP_S => Some((a, n.end)),
            Some((a, b)) => {
                spans.push(Span { start: a, end: b, kind: "spoken".into(), extra: Map::new() });
                Some((n.start, n.end))
            }
            None => Some((n.start, n.end)),
        };
    }
    if let Some((a, b)) = run {
        spans.push(Span { start: a, end: b, kind: "spoken".into(), extra: Map::new() });
    }
    // Tuning, decided once in the file from the stored (rounded) values.
    let cents = round_to(g.tuning_cents.clamp(-50.0, 50.0), 1);
    let strength = round3(g.tuning_strength.clamp(0.0, 1.0));
    let use_cents = ctx.cfg.use_cents(cents, strength);
    let mut parameters = match serde_json::to_value(ctx.cfg) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    // f32 fields serialise with float noise (0.4 -> 0.4000000059604645); store them as
    // the decimal the code means.
    for k in ["low_confidence", "octave_flag_factor"] {
        if let Some(x) = parameters.get(k).and_then(Value::as_f64) {
            parameters.insert(k.into(), Value::from(round_to(x, 6)));
        }
    }
    let mut doc = GuideDoc {
        guide_version: GUIDE_VERSION,
        hop_s: ctx.cfg.hop_s,
        low_confidence: round_to(ctx.cfg.low_confidence as f64, 6),
        tuning: Tuning { cents, strength, use_cents, extra: Map::new() },
        range: None,
        counts: None,
        curve: Some(CURVE_CHUNK.into()),
        notes,
        spans,
        words: None,
        provenance: GuideProvenance {
            method: METHOD.into(),
            version: VERSION.into(),
            parameters,
            source: Some(ctx.source.clone()),
            analysis_rate_hz: Some(round_to(ctx.analysis_rate_hz, 3)),
            app: Some(ctx.app.clone()),
            created_utc: Some(ctx.created_utc.clone()),
            seconds: Some(round_to(ctx.seconds, 3)),
            extra: Map::new(),
        },
        extra: Map::new(),
    };
    doc.range = doc.compute_range();
    doc.counts = Some(doc.compute_counts());
    // The curve: exactly floor(duration / hop) + 1 frames, 0 = unvoiced, never NaN.
    let hop_us = (ctx.cfg.hop_s * 1e6).round().max(1.0) as u32;
    let n = Curve::frames_for(ctx.duration_s, hop_us);
    let c = &g.curve;
    let mut midi = vec![0.0f32; n];
    let mut voic = vec![0.0f32; n];
    let mut lvdb = vec![-120.0f32; n];
    for k in 0..n.min(c.len()) {
        let f = c.f0_hz[k] as f64;
        if f.is_finite() && f > 0.0 {
            let m = jk_pitch::guide::hz_to_midi(f);
            if (12.0..=120.0).contains(&m) {
                midi[k] = m as f32;
            }
        }
        let v = c.conf[k];
        voic[k] = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let l = c.level_db.get(k).copied().unwrap_or(-120.0);
        lvdb[k] = if l.is_finite() { l.max(-120.0) } else { -120.0 };
    }
    (doc, Curve::new(hop_us, midi, voic, lvdb))
}

/// Section 10.4: attaches each note to a word of the effective lyrics by the overlap
/// rule, and records what the mapping was made against. Never moves a note.
pub fn set_word_refs(doc: &mut GuideDoc, lyrics: Option<&Lyrics>, is_override: bool, offset_ms: i64) {
    for n in doc.notes.iter_mut() {
        n.word = None;
    }
    let Some(l) = lyrics else {
        doc.words = None;
        return;
    };
    let off = offset_ms as f64 / 1000.0;
    let words: Vec<WordSpan> = l
        .lines
        .iter()
        .enumerate()
        .flat_map(|(li, line)| {
            line.words
                .iter()
                .enumerate()
                .map(move |(wi, w)| WordSpan { line: li, word: wi, start: w.start + off, end: w.end + off })
        })
        .collect();
    let spans: Vec<(f64, f64)> = doc.notes.iter().map(|n| (n.start, n.end)).collect();
    for (n, m) in doc.notes.iter_mut().zip(map_words(&spans, &words)) {
        n.word = m.map(|i| WordRef(words[i].line, words[i].word, round3(words[i].start)));
    }
    doc.words = Some(WordsRef {
        lyrics: if is_override { "override".into() } else { "chunk".into() },
        offset_ms,
        word_count: words.len(),
        extra: Map::new(),
    });
}

/// The lead vocal stem of an open song: its manifest entry and TOC entry.
fn lead_of(song: &SongFile) -> Result<(jk_format::StemInfo, jk_format::ChunkEntry), GuideError> {
    let stem = song.manifest().stem("lead_vocal").cloned().ok_or_else(GuideError::no_lead)?;
    let entry = song.chunk(&stem.chunk).cloned().ok_or_else(GuideError::no_lead)?;
    Ok((stem, entry))
}

/// Step 1 of section 7.1: builds the guide from the song file itself (its `lead_vocal`
/// stem and its effective lyrics). The file is only read, so the song may keep playing.
pub fn build_for_song(path: &Path, cfg: &GuideConfig, app: &GuideApp, ctl: &mut BuildControl<'_>) -> Result<Built, GuideError> {
    let t0 = Instant::now();
    let song = SongFile::open(path).map_err(|e| read_error(&e))?;
    let (stem, entry) = lead_of(&song)?;
    ctl.check()?;
    // Pass 1: CRC-32 (the TOC's) and SHA-256 of the chunk's bytes as stored.
    let mut r = song.chunk_reader(&stem.chunk).map_err(|e| read_error(&e))?;
    let mut sha = sha2::Sha256::new();
    let mut crc = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        ctl.check()?;
        let n = r.read(&mut buf).map_err(|e| read_error(&FormatError::Io(e)))?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        crc.update(&buf[..n]);
        done += n as u64;
        ctl.report(0.15 * done as f64 / entry.length.max(1) as f64);
    }
    if done != entry.length || crc.finalize() != entry.crc32 {
        return Err(GuideError::damaged(format!("chunk '{}' checksum mismatch", stem.chunk)));
    }
    let sha256 = hex(&sha.finalize());
    // Pass 2: decode, stored gain applied, channels averaged (a lead stem, not a
    // microphone: SONG_FORMAT 10.5).
    let reader = song.chunk_reader(&stem.chunk).map_err(|e| read_error(&e))?;
    let mut dec = jk_format::decode::StemDecoder::new(reader, false).map_err(|e| GuideError::damaged(e.to_string()))?;
    let ch = dec.channels.max(1);
    let rate = dec.sample_rate;
    let scale = stem.gain / 2f64.powi(dec.bits_per_sample as i32 - 1) / ch as f64;
    let total = song.manifest().duration_frames.max(1);
    let mut mono: Vec<f32> = Vec::with_capacity(total as usize);
    let mut block = Vec::new();
    while let Some((_, frames)) = dec.next_block(&mut block).map_err(|e| GuideError::damaged(e.to_string()))? {
        for f in 0..frames {
            let s: i64 = block[f * ch..(f + 1) * ch].iter().map(|&x| x as i64).sum();
            mono.push((s as f64 * scale) as f32);
        }
        if mono.len() % (rate as usize * 10) < frames {
            ctl.check()?;
            ctl.report(0.15 + 0.25 * mono.len() as f64 / total as f64);
        }
    }
    ctl.check()?;
    // The builder (pure, single-threaded).
    let a = jk_pitch::guide::prepare(&mono, 1, rate);
    drop(mono);
    ctl.report(0.45);
    let curve = jk_pitch::guide::pitch_curve(&a, cfg);
    ctl.check()?;
    ctl.report(0.85);
    let g = jk_pitch::guide::segment(curve, &a, cfg);
    ctl.report(0.95);
    let seconds = t0.elapsed().as_secs_f64();
    let m = song.manifest();
    let ctx = DocContext {
        duration_s: m.duration(),
        cfg,
        source: GuideSource {
            role: "lead_vocal".into(),
            chunk: stem.chunk.clone(),
            length: entry.length,
            crc32: entry.crc32,
            sha256,
            sample_rate: m.sample_rate,
            channels: m.channels,
            extra: Map::new(),
        },
        analysis_rate_hz: a.sample_rate,
        app: app.clone(),
        created_utc: jk_format::time::utc_now_iso8601(),
        seconds,
    };
    let (mut doc, curve) = doc_from_guide(&g, &ctx);
    // Word references against the effective lyrics (a damaged lyrics chunk only costs
    // the references, never the guide).
    let lyrics = song.effective_lyrics().ok().flatten();
    let is_override = song.user_state().lyrics_override.is_some();
    set_word_refs(&mut doc, lyrics.as_ref(), is_override, song.effective_settings().lyrics_offset_ms);
    ctl.report(1.0);
    Ok(Built { doc, curve, seconds })
}

/// Step 2: writes the temporary file (the song may keep playing meanwhile).
pub fn prepare_write(path: &Path, built: &Built, ctl: &mut RewriteControl<'_>) -> Result<PreparedGuide, GuideError> {
    let chunks = built.chunks();
    let needed = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) + (chunks.guide_json.len() + chunks.curve.len()) as u64;
    jk_format::guide_write::prepare_guide_rewrite(path, &chunks, ctl).map_err(|e| write_error(&e, WriteStage::Prepare, needed))
}

/// Steps 3-5: the checks and the swap (the caller has released the song).
pub fn finish_write(p: PreparedGuide, ctl: &mut RewriteControl<'_>) -> Result<(), GuideError> {
    let needed = p.file_length;
    p.finish(ctl).map(|_| ()).map_err(|e| write_error(&e, WriteStage::Swap, needed))
}

/// The guide's state in the file for this app's builder (SONG_FORMAT 10.2).
pub fn file_state(song: &SongFile) -> (GuideFileState, Option<GuideDoc>) {
    jk_format::guide::guide_state(song, METHOD, VERSION)
}

/// Whether `guide add` builds: none, stale or outdated (`force`: also ready). A `newer`
/// guide is never overwritten, and a guide from a newer version of this builder is never
/// downgraded, not even with `force`.
pub fn wants_build(state: &GuideFileState, doc: Option<&GuideDoc>, force: bool) -> bool {
    match state {
        GuideFileState::None | GuideFileState::Stale(_) | GuideFileState::Outdated => true,
        GuideFileState::Newer => false,
        GuideFileState::Ready => {
            let newer_builder = doc.is_some_and(|d| {
                d.provenance.method == METHOD
                    && jk_format::guide::compare_versions(&d.provenance.version, VERSION) == std::cmp::Ordering::Greater
            });
            force && !newer_builder
        }
    }
}

/// The state's name as in `GuideStatus.state` (docs/APP_API.md 12.3).
pub fn state_name(s: &GuideFileState) -> &'static str {
    match s {
        GuideFileState::None => "none",
        GuideFileState::Ready => "ready",
        GuideFileState::Outdated => "outdated",
        GuideFileState::Stale(_) => "stale",
        GuideFileState::Newer => "newer",
    }
}

/// What [`add_to_song`] did.
#[derive(Clone, Debug)]
pub struct AddOutcome {
    /// The state before.
    pub before: GuideFileState,
    /// The guide written, or `None` when nothing needed doing.
    pub built: Option<Built>,
    /// Seconds for the whole operation (build + rewrite).
    pub total_seconds: f64,
}

/// Builds and writes in one go, for a song nobody else has open (`jukeoke-cli guide add`).
/// Skips songs whose guide is current unless `force`.
pub fn add_to_song(path: &Path, cfg: &GuideConfig, app: &GuideApp, force: bool) -> Result<AddOutcome, GuideError> {
    let t0 = Instant::now();
    let (before, doc) = {
        let song = SongFile::open(path).map_err(|e| read_error(&e))?;
        let (s, d) = file_state(&song);
        // A song without a lead vocal fails here rather than after a no-op check.
        lead_of(&song)?;
        (s, d)
    };
    if !wants_build(&before, doc.as_ref(), force) {
        return Ok(AddOutcome { before, built: None, total_seconds: t0.elapsed().as_secs_f64() });
    }
    let built = build_for_song(path, cfg, app, &mut BuildControl::default())?;
    let mut ctl = RewriteControl::default();
    let p = prepare_write(path, &built, &mut ctl)?;
    finish_write(p, &mut ctl)?;
    Ok(AddOutcome { before, built: Some(built), total_seconds: t0.elapsed().as_secs_f64() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jk_pitch::guide::{Guide, Note, PitchCurve};

    fn note(a: f64, b: f64, m: f64, kind: NoteKind) -> Note {
        Note { start_s: a, end_s: b, midi: m, confidence: 0.9, kind, octave: OctaveCheck::Ok, glide_in_s: 0.0 }
    }

    fn ctx(cfg: &GuideConfig) -> DocContext<'_> {
        DocContext {
            duration_s: 4.0,
            cfg,
            source: GuideSource {
                role: "lead_vocal".into(),
                chunk: "stem.lead_vocal".into(),
                length: 10,
                crc32: 1,
                sha256: "ab".into(),
                sample_rate: 48_000,
                channels: 2,
                extra: Map::new(),
            },
            analysis_rate_hz: 16_000.0,
            app: app_stamp("Open Mic", "0.1.0"),
            created_utc: "2026-11-19T00:00:00Z".into(),
            seconds: 0.01,
        }
    }

    #[test]
    fn conversion_rounds_builds_spans_and_applies_the_tuning_rule() {
        let cfg = GuideConfig::default();
        let g = Guide {
            method: METHOD,
            version: VERSION,
            notes: vec![
                note(0.10049, 0.5004, 60.12345, NoteKind::Pitched),
                note(1.0, 1.2, 55.0, NoteKind::Unpitched),
                note(1.3, 1.5, 0.0, NoteKind::Unpitched),
                note(2.0, 2.2, 57.0, NoteKind::Unpitched),
                note(3.0, 4.2, 62.0, NoteKind::Pitched),
            ],
            tuning_cents: -15.86,
            tuning_strength: 0.2999,
            octave_flags: 0,
            curve: PitchCurve { hop_s: 0.01, f0_hz: vec![261.63; 350], conf: vec![0.8; 350], level_db: vec![-20.0; 350] },
        };
        let (doc, curve) = doc_from_guide(&g, &ctx(&cfg));
        assert_eq!((doc.notes[0].start, doc.notes[0].end, doc.notes[0].midi), (0.1, 0.5, 60.123));
        assert_eq!(doc.notes[4].end, 4.0, "clamped to the song");
        assert_eq!(doc.spans.len(), 2, "1.0-1.5 is one spoken span, 2.0-2.2 another: {:?}", doc.spans);
        assert_eq!((doc.spans[0].start, doc.spans[0].end), (1.0, 1.5));
        assert_eq!(doc.tuning.cents, -15.9);
        // The rule is applied to the stored (rounded) values: 0.2999 is stored as 0.3.
        assert_eq!((doc.tuning.strength, doc.tuning.use_cents), (0.3, -15.9));
        let weak = Guide { tuning_strength: 0.2994, ..g.clone() };
        let (d2, _) = doc_from_guide(&weak, &ctx(&cfg));
        assert_eq!((d2.tuning.strength, d2.tuning.use_cents), (0.299, 0.0), "below 0.3: A = 440");
        assert!(jk_format::guide::validate_doc(&doc, 4.0).errors.is_empty());
        assert_eq!(curve.frames(), 401);
        assert!((curve.midi().unwrap()[10] - 60.0).abs() < 0.01);
        assert_eq!(curve.midi().unwrap()[360], 0.0, "beyond the builder's curve: unvoiced");
        assert!(jk_format::guide::validate_curve(&curve, 4.0).errors.is_empty());
        assert_eq!(doc.provenance.parameters["tuning_min_strength"], Value::from(0.3));
        assert_eq!(doc.provenance.parameters["low_confidence"], Value::from(0.5));
    }

    #[test]
    fn write_errors_map_to_plain_codes() {
        use std::io::{Error, ErrorKind as K};
        let io = |e: Error| FormatError::Io(e);
        // Raw OS codes of this system: (disk full, write-protected, in use).
        #[cfg(windows)]
        let (full, ro, busy) = (112, 19, Some(32));
        #[cfg(unix)]
        let (full, ro, busy) = (28, 30, None::<i32>);
        assert_eq!(write_error(&io(Error::from_raw_os_error(full)), WriteStage::Prepare, 150 << 20).code, "guide_disk_full");
        assert!(write_error(&io(Error::from_raw_os_error(full)), WriteStage::Prepare, 150 << 20).message.contains("151 MB"));
        assert_eq!(write_error(&io(Error::from_raw_os_error(ro)), WriteStage::Prepare, 1).code, "guide_read_only");
        if let Some(b) = busy {
            assert_eq!(write_error(&io(Error::from_raw_os_error(b)), WriteStage::Swap, 1).code, "guide_in_use");
        }
        #[cfg(unix)]
        assert_eq!(write_error(&io(Error::from_raw_os_error(32)), WriteStage::Swap, 1).code, "guide_internal", "EPIPE is not 'in use'");
        assert_eq!(write_error(&io(Error::from(K::PermissionDenied)), WriteStage::Prepare, 1).code, "guide_read_only");
        assert_eq!(write_error(&io(Error::from(K::PermissionDenied)), WriteStage::Swap, 1).code, "guide_in_use");
        assert_eq!(write_error(&jk_format::guide_write::cancelled_error(), WriteStage::Swap, 1).code, "cancelled");
        assert_eq!(write_error(&FormatError::ChangedOnDisk, WriteStage::Swap, 1).code, "guide_internal");
    }
}
