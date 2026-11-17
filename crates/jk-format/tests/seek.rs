//! Seeking inside FLAC stems is sample-accurate over long spans, on real song files.
//!
//! Regression for symphonia-bundle-flac 0.6.1: its packet parser keeps state from before
//! a seek when the seek lands exactly on a frame start (a SEEKTABLE point, the first
//! frame). The first packet after such a seek decodes right, the packets after it can
//! decode to wrong samples without any error. A check that compares only a few frames
//! after each seek cannot see that, so every seek here is compared over at least one
//! second (dozens of FLAC frames) against a full sequential decode, bit for bit, with
//! one decoder reused across all seeks (forwards, backwards, to 0, to seek points).
//!
//! Fixtures: `tests/fixtures/generated/demo-song.jksong` (jk-engine's FLAC encoder,
//! 40 s mono) and `tiny.jksong` (libsndfile, like the Studio, 4 s stereo). The
//! 4-minute generated song is in `crates/jk-engine/tests/song_seek.rs` (the generator
//! lives in jk-engine).

use std::path::PathBuf;

use jk_format::decode::StemDecoder;
use jk_format::{SongFile, VerifyOptions, verify_file};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/generated").join(name)
}

/// Whole stem, decoded front to back (interleaved integers), MD5 checked.
fn sequential(song: &SongFile, role: &str) -> (Vec<i32>, usize, u32) {
    let (_, rd) = song.stem_reader(role).unwrap();
    let mut dec = StemDecoder::new(rd, true).unwrap();
    let mut all = Vec::new();
    let mut block = Vec::new();
    let mut expected_ts = 0u64;
    while let Some((ts, n)) = dec.next_block(&mut block).unwrap() {
        assert_eq!(ts, expected_ts, "{role}: packets are contiguous");
        expected_ts += n as u64;
        all.extend_from_slice(&block);
    }
    assert_eq!(dec.finish(), Some(true), "{role}: STREAMINFO MD5");
    (all, dec.channels, dec.sample_rate)
}

/// After `dec.seek(target)`, collect `frames` frames starting exactly at `target`.
fn span_after_seek(dec: &mut StemDecoder, target: u64, frames: usize) -> Vec<i32> {
    let ch = dec.channels;
    let landed = dec.seek(target).unwrap();
    assert!(landed <= target, "seek to {target} landed after it, at {landed}");
    let mut got = Vec::with_capacity(frames * ch);
    let mut block = Vec::new();
    while got.len() < frames * ch {
        let Some((ts, n)) = dec.next_block(&mut block).unwrap() else { break };
        let next = target + (got.len() / ch) as u64;
        assert!(ts <= next, "seek to {target}: packet at {ts} skips past frame {next}");
        if next < ts + n as u64 {
            let from = (next - ts) as usize;
            let take = (frames - got.len() / ch).min(n - from);
            got.extend_from_slice(&block[from * ch..(from + take) * ch]);
        }
    }
    got
}

/// Deterministic seek targets: edges (0 and frame 1, around block boundaries, the
/// last frames), repeated returns to 0, and pseudo-random positions, in an order that
/// mixes backward and forward jumps.
pub fn seek_targets(total: u64, span: u64, random: usize, seed: u64) -> Vec<u64> {
    let mut v = vec![0, 1, 4095, 4096, 4097, total / 2, 0, total.saturating_sub(span), total - 1, 0];
    let mut s = seed;
    let hi = total.saturating_sub(span).max(1);
    for i in 0..random {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        v.push((s >> 33) % hi);
        if i % 10 == 9 {
            v.push(0);
        }
    }
    v
}

/// Seeks with one reused decoder, each compared over `span` frames (or to the end).
/// Returns (seeks checked, frames compared).
fn check_song(path: &PathBuf, span_s: f64, random: usize) -> (usize, u64) {
    let song = SongFile::open(path).unwrap();
    let mut seeks = 0;
    let mut compared = 0u64;
    for stem in &song.manifest().stems {
        let (all, ch, rate) = sequential(&song, &stem.role);
        let total = (all.len() / ch) as u64;
        assert_eq!(total, song.manifest().duration_frames, "{}: length", stem.role);
        let span = (span_s * rate as f64).ceil() as u64;
        let (_, rd) = song.stem_reader(&stem.role).unwrap();
        let mut dec = StemDecoder::new(rd, false).unwrap();
        let mut bad = Vec::new();
        for t in seek_targets(total, span, random, 0x5eed ^ seeks as u64) {
            let want_frames = span.min(total - t) as usize;
            let got = span_after_seek(&mut dec, t, want_frames);
            let want = &all[t as usize * ch..(t as usize + want_frames) * ch];
            if got.len() != want.len() {
                bad.push(format!("seek to {t}: got {} frames, expected {want_frames}", got.len() / ch));
            } else if let Some(i) = got.iter().zip(want).position(|(a, b)| a != b) {
                let wrong = got.iter().zip(want).filter(|(a, b)| a != b).count();
                bad.push(format!("seek to {t}: first wrong sample at frame +{}, {wrong} of {} samples wrong", i / ch, want.len()));
            }
            seeks += 1;
            compared += want_frames as u64;
        }
        assert!(bad.is_empty(), "{}: stem '{}': {} bad seeks:\n{}", path.display(), stem.role, bad.len(), bad.join("\n"));
    }
    (seeks, compared)
}

#[test]
fn demo_song_seeks_match_sequential_decode_over_one_second() {
    let (seeks, frames) = check_song(&fixture("demo-song.jksong"), 1.0, 40);
    println!("demo-song.jksong: {seeks} seeks bit-exact, {frames} frames compared ({:.1} s of audio)", frames as f64 / 44_100.0);
}

#[test]
fn libsndfile_stems_seek_match_sequential_decode_over_one_second() {
    let (seeks, frames) = check_song(&fixture("tiny.jksong"), 1.0, 20);
    println!("tiny.jksong: {seeks} seeks bit-exact, {frames} frames compared");
}

/// `verify` compares at least one second after each seek (it used to compare 64
/// frames, which fits inside the first FLAC frame and so could not see the bug).
#[test]
fn verify_seek_check_covers_a_second_per_seek() {
    for name in ["demo-song.jksong", "tiny.jksong"] {
        let rep = verify_file(fixture(name), &VerifyOptions { seek_checks: 16, ..Default::default() });
        assert!(rep.errors.is_empty(), "{name}: {:?}", rep.errors);
        for s in &rep.stems {
            let (ok, tried) = s.seeks_ok.unwrap();
            assert!(tried >= 15 && ok == tried, "{name} {}: {ok}/{tried}", s.role);
            assert!(s.seek_frames_compared.unwrap() >= tried as u64 * 44_100 - 44_100, "{name} {}: {:?}", s.role, s.seek_frames_compared);
        }
    }
}
