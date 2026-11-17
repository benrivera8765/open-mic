// SPDX-License-Identifier: GPL-3.0-or-later
//! Random seeks into a real 4-minute generated song, bit-exact over one second.
//!
//! The song: the four 24-bit stereo stems of `jk_dsp::signals::synth_song` (what
//! `jukeoke-cli make-test-song --seconds 240` writes), arranged like a real song's stems:
//! the lead vocal is digitally silent in the intro, two instrumental breaks and the
//! outro, the backing vocals sing only the choruses, the drums enter after the intro and
//! drop out for a breakdown. Encoded with this crate's FLAC encoder (a SEEKTABLE point
//! every second, as in every song file it writes) and written with jk-format's song
//! writer. The silences matter: silent FLAC frames are a few bytes, so a whole second
//! between two seek points fits in symphonia's linear-search window and a seek there
//! lands exactly on a frame start without moving the parser, which is the case that left
//! symphonia-bundle-flac 0.6.1 with stale state. (A song without silences, like the plain
//! `make-test-song` output, does not trigger it: there every seek binary-searches through
//! the middle of a frame, which resets the parser.)
//!
//! Each stem is decoded front to back once and compared with the generator's own samples
//! (and the STREAMINFO MD5); then both stem decoders seek around in it, one decoder per
//! stem reused across all seeks, and every seek is compared over 1 s (about 11 FLAC
//! frames; less only for the targets inside the last second) against that sequential
//! decode:
//!
//! * `jk_format::decode::StemDecoder` (verify, export check, tools), and
//! * `jk_engine::FlacStem` on song-file chunk views, exactly as `SongSource` builds it
//!   (playback, export render).
//!
//! Targets: pseudo-random positions, exact SEEKTABLE points and frame 0, block edges,
//! the last frames; shuffled, so jumps go forwards and backwards.

// Index loops over channels and frames are the clearest form for audio code.
#![allow(clippy::needless_range_loop)]

use std::path::PathBuf;

use jk_engine::FlacStem;
use jk_format::decode::StemDecoder;
use jk_format::{ChunkData, Manifest, SongFile, SongWriter, StemInfo, UserState};

const SECONDS: f64 = 240.0;
const RATE: u32 = 44_100;
const SEED: u32 = 20_261_117;
/// Frames compared after every seek: one second.
const SPAN: usize = RATE as usize;

/// Stem roles in `synth_song` order, and where each stem is digitally silent (seconds).
const ARRANGEMENT: [(&str, &[(f64, f64)]); 4] = [
    ("lead_vocal", &[(0.0, 16.0), (64.0, 80.0), (144.0, 168.0), (224.0, 240.0)]),
    ("backing_vocals", &[(0.0, 32.0), (48.0, 96.0), (112.0, 184.0), (200.0, 240.0)]),
    ("drums", &[(0.0, 8.0), (128.0, 144.0), (232.0, 240.0)]),
    ("other", &[(238.0, 240.0)]),
];

fn quantise(x: f32) -> i32 {
    (x as f64 * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32
}

/// The arranged stem `k` of the generated song: interleaved 24-bit integers.
fn arranged(stems: &[[Vec<f32>; 2]; 4], k: usize) -> Vec<i32> {
    let frames = stems[k][0].len();
    let mut v = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / RATE as f64;
        let silent = ARRANGEMENT[k].1.iter().any(|&(a, b)| t >= a && t < b);
        for c in 0..2 {
            v.push(if silent { 0 } else { quantise(stems[k][c][i]) });
        }
    }
    v
}

/// Writes the arranged 4-minute song. Returns its path and length in frames.
fn write_song(stems: &[[Vec<f32>; 2]; 4]) -> (PathBuf, u64) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jk-engine-song-seek");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("arranged-240s.jksong");
    let frames = stems[0][0].len() as u64;
    let mut infos = Vec::new();
    let mut flacs = Vec::new();
    for (k, (role, _)) in ARRANGEMENT.iter().enumerate() {
        let ints = arranged(stems, k);
        let peak = ints.iter().map(|&v| (v as f64 / 8_388_608.0).abs()).fold(0.0, f64::max);
        flacs.push((format!("stem.{role}"), jk_engine::encode_flac(&ints, 2, 24, RATE)));
        infos.push(StemInfo {
            role: role.to_string(),
            chunk: format!("stem.{role}"),
            codec: "flac".into(),
            bits: 24,
            gain: 1.0,
            peak: Some(peak),
            in_karaoke_mix: Some(*role != "lead_vocal"),
            pitched: Some(*role != "drums"),
            extra: Default::default(),
        });
    }
    let manifest = Manifest {
        format: "jksong".into(),
        format_version: jk_format::FORMAT_VERSION.into(),
        title: "Arranged synthetic song (240 s)".into(),
        artist: "Generated".into(),
        album: None,
        sample_rate: RATE,
        channels: 2,
        duration_frames: frames,
        duration_seconds: frames as f64 / RATE as f64,
        cover: None,
        lyrics: None,
        analysis: None,
        preset: Some("full".into()),
        stems_sum_to_source: true,
        stems: infos,
        provenance: serde_json::json!({ "generator": { "tool": "jk-engine tests/song_seek.rs", "seed": SEED } }),
        extra: Default::default(),
    };
    let mut w = SongWriter::new(manifest).user_state(UserState::default());
    for (id, bytes) in flacs {
        w.add_chunk(id, "audio/flac", ChunkData::Bytes(bytes)).unwrap();
    }
    w.write_atomic(&p).unwrap();
    (p, frames)
}

/// Seek targets for one stem: exact seek points (every 3rd), random positions, frame 0
/// every few seeks, edges. Shuffled so jumps go both ways.
fn targets(total: u64, seek_points: &[u64], seed: u64) -> Vec<u64> {
    let mut s = seed;
    let mut next = move || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        s >> 33
    };
    let hi = total - SPAN as u64;
    let mut v: Vec<u64> = seek_points.iter().step_by(3).copied().filter(|&p| p <= hi).collect();
    v.extend((0..60).map(|_| next() % hi));
    v.extend([1, 4095, 4096, 4097, hi, hi + 1]);
    // Fisher-Yates with the same generator.
    for i in (1..v.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    // Back to frame 0 regularly (lands exactly on the first frame), and first of all,
    // straight after the full decode has read to the end of the stream.
    let mut out = vec![0];
    for (k, t) in v.into_iter().enumerate() {
        out.push(t);
        if k % 8 == 7 {
            out.push(0);
        }
    }
    out.extend([total - 1, 0]);
    out
}

/// A FLAC sample as the engine reads it: integer / 2^23.
fn f(x: i32) -> f32 {
    x as f32 / 8_388_608.0
}

#[test]
fn four_minute_song_random_seeks_are_bit_exact_over_one_second() {
    let t0 = std::time::Instant::now();
    let stems = jk_dsp::signals::synth_song(RATE, SECONDS, SEED);
    let (path, frames) = write_song(&stems);
    let made_s = t0.elapsed().as_secs_f64();
    assert_eq!(frames, (SECONDS * RATE as f64) as u64);
    let file = std::sync::Arc::new(SongFile::open(&path).unwrap());

    let mut total_seeks = 0usize;
    let mut total_frames = 0u64;
    let mut exact_points = 0usize;
    let mut bad: Vec<String> = Vec::new();
    for (k, stem) in file.manifest().stems.clone().iter().enumerate() {
        let role = stem.role.as_str();
        assert_eq!(role, ARRANGEMENT[k].0);
        // 1. Sequential decode: contiguous packets, STREAMINFO MD5, and exactly the
        //    generated samples.
        let (_, rd) = file.stem_reader(role).unwrap();
        let mut dec = StemDecoder::new(rd, true).unwrap();
        assert_eq!((dec.channels, dec.sample_rate, dec.bits_per_sample), (2, RATE, 24));
        let mut all: Vec<i32> = Vec::with_capacity(frames as usize * 2);
        let mut block = Vec::new();
        while let Some((ts, _)) = dec.next_block(&mut block).unwrap() {
            assert_eq!(ts as usize * 2, all.len(), "{role}: contiguous packets");
            all.extend_from_slice(&block);
        }
        assert_eq!(all.len(), frames as usize * 2, "{role}: length");
        assert_eq!(dec.finish(), Some(true), "{role}: STREAMINFO MD5");
        assert!(all == arranged(&stems, k), "{role}: decoded samples differ from the generated ones");

        // Seek points of this stem (exact frame starts).
        let (_, rd) = file.stem_reader(role).unwrap();
        let meta = jk_format::flac::read_metadata(rd).unwrap();
        let points: Vec<u64> = meta.seek_points.unwrap().iter().map(|p| p.sample).collect();
        assert!(points.len() >= 239, "{role}: {} seek points", points.len());
        let list = targets(frames, &points, 0x9e37_79b9 ^ k as u64);
        exact_points += list.iter().filter(|t| points.contains(t)).count();

        let bad_before = bad.len();
        // 2. jk-format StemDecoder: the same decoder that has just read the whole stream.
        for &t in &list {
            let n = SPAN.min((frames - t) as usize);
            let mut got: Vec<i32> = Vec::with_capacity(n * 2);
            let result = (|| -> jk_format::Result<()> {
                let landed = dec.seek(t)?;
                assert!(landed <= t, "{role}: seek to {t} landed at {landed}");
                while got.len() < n * 2 {
                    let Some((ts, m)) = dec.next_block(&mut block)? else { break };
                    let want_next = t + got.len() as u64 / 2;
                    if ts > want_next {
                        break; // frames skipped: reported below as a short read
                    }
                    if want_next < ts + m as u64 {
                        let from = (want_next - ts) as usize;
                        let take = (n - got.len() / 2).min(m - from);
                        got.extend_from_slice(&block[from * 2..(from + take) * 2]);
                    }
                }
                Ok(())
            })();
            let want = &all[t as usize * 2..(t as usize + n) * 2];
            if let Err(e) = result {
                bad.push(format!("StemDecoder seek to {t}: error after {} frames: {e}", got.len() / 2));
            } else if got != want {
                let first = got.iter().zip(want).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
                bad.push(format!("StemDecoder seek to {t}: {} of {n} frames, first difference at +{}", got.len() / 2, first / 2));
            }
            total_frames += n as u64;
        }

        // 3. jk-engine FlacStem on chunk views of the song file (as SongSource opens it).
        let (file2, chunk) = (file.clone(), stem.chunk.clone());
        let factory: jk_engine::decode::SourceFactory = Box::new(move || {
            file2
                .chunk_reader(&chunk)
                .map(|r| Box::new(r) as Box<dyn symphonia::core::io::MediaSource>)
                .map_err(|e| std::io::Error::other(e.to_string()))
        });
        let mut fs = FlacStem::from_factory(factory, &PathBuf::from(format!("{}#{}", path.display(), stem.chunk))).unwrap();
        let mut buf = [vec![0.0f32; SPAN], vec![0.0f32; SPAN]];
        // Read it all once first, so the first seek starts from the end of the stream.
        while fs.read(&mut buf, 0, SPAN).unwrap() > 0 {}
        for &t in &list {
            let n = SPAN.min((frames - t) as usize);
            fs.seek(t).unwrap();
            let mut got = 0;
            while got < n {
                let m = fs.read(&mut buf, got, n - got).unwrap();
                if m == 0 {
                    break;
                }
                got += m;
            }
            let first = (0..n).find(|&i| {
                let src = t as usize + i;
                i >= got || buf[0][i] != f(all[src * 2]) || buf[1][i] != f(all[src * 2 + 1])
            });
            if let Some(i) = first {
                bad.push(format!("FlacStem seek to {t}: {got} of {n} frames, first difference at +{i}"));
            }
            total_frames += n as u64;
        }
        for b in &mut bad[bad_before..] {
            b.insert_str(0, &format!("{role}: "));
        }
        total_seeks += 2 * list.len();
    }
    // 4. `jukeoke-cli verify`'s checklist on the same file (its seeks compare 1 s each).
    let verify = jk_format::verify_file(&path, &Default::default());
    let verify_seeks: Vec<_> = verify.stems.iter().map(|s| (s.role.as_str(), s.seeks_ok, s.seek_frames_compared)).collect();
    println!("verify: {} errors; seeks (ok, tried) and frames compared per stem: {verify_seeks:?}", verify.errors.len());
    assert!(bad.is_empty() && verify.errors.is_empty(), "{} of {} seeks wrong:\n{}\nverify: {:?}", bad.len(), total_seeks, bad.join("\n"), verify.errors);
    println!(
        "4-minute song ({frames} frames, {} bytes, made in {made_s:.1} s): {total_seeks} seeks bit-exact (2 decoders x 4 stems; {exact_points} of the {} targets are exact seek points incl. frame 0), {total_frames} frames ({:.0} s of audio) compared, total {:.1} s",
        std::fs::metadata(&path).unwrap().len(),
        total_seeks / 2,
        total_frames as f64 / RATE as f64,
        t0.elapsed().as_secs_f64()
    );
}
