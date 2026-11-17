// SPDX-License-Identifier: GPL-3.0-or-later
//! Diagnostic: decode a FLAC file packet by packet with symphonia and report where it
//! stops. Run: cargo run --release -p jk-engine --example flac_probe -- <file.flac>
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = symphonia::default::get_probe().probe(&Hint::new(), mss, FormatOptions::default(), MetadataOptions::default()).unwrap();
    let track = format.default_track(TrackType::Audio).unwrap();
    println!("num_frames {:?}", track.num_frames);
    let params = track.codec_params.as_ref().unwrap().audio().unwrap().clone();
    let tid = track.id;
    let mut dec = symphonia::default::get_codecs().make_audio_decoder(&params, &AudioDecoderOptions::default()).unwrap();
    let mut total = 0u64; let mut n = 0;
    loop {
        match format.next_packet() {
            Ok(Some(p)) => {
                if p.track_id != tid { continue; }
                n += 1;
                match dec.decode(&p) { Ok(b) => total += b.frames() as u64, Err(e) => { println!("decode error at packet {n} pts {} : {e:?}", p.pts.get()); break; } }
            }
            Ok(None) => { println!("end: Ok(None)"); break; }
            Err(e) => { println!("next_packet error after {n} packets, {total} frames: {e:?}"); break; }
        }
    }
    println!("packets {n}, frames {total}");
}
