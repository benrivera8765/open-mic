//! Runs the SONG_FORMAT.md section 9 checklist on a song file, decoding every stem
//! with symphonia (rate, channels, bits, length, MD5, sample-accurate seeks).
//!
//! ```text
//! cargo run -p jk-format --example jksong-verify -- <file.jksong>
//!     [--strict]                  warnings fail too
//!     [--sum-reference <raw>]     raw float32 little-endian interleaved source audio:
//!                                 report how far the sum of all stems at unity is from it
//!     [--max-sum-error <x>]       fail if that max abs error exceeds x (default: the
//!                                 24-bit quantisation bound, see `sum_check`)
//!     [--exact-residual <role>]   the file was written with the residual stem computed
//!                                 from the decoded others: limit = half an LSB of that
//!                                 stem (0.5 * gain / 8388608)
//!     [--expect <json>]           expected effective values (see `check_expect`)
//! ```
//! Exit status: 0 pass, 1 fail, 2 usage or I/O error.

use std::path::PathBuf;
use std::process::ExitCode;

use jk_format::decode::StemDecoder;
use jk_format::{SongFile, VerifyOptions, verify_file};
use serde_json::{Value, json};

struct Args {
    file: PathBuf,
    strict: bool,
    sum_reference: Option<PathBuf>,
    max_sum_error: Option<f64>,
    exact_residual: Option<String>,
    expect: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        file: PathBuf::new(),
        strict: false,
        sum_reference: None,
        max_sum_error: None,
        exact_residual: None,
        expect: None,
    };
    let mut file = None;
    while let Some(x) = it.next() {
        match x.as_str() {
            "--strict" => a.strict = true,
            "--sum-reference" => a.sum_reference = Some(it.next().ok_or("--sum-reference needs a path")?.into()),
            "--max-sum-error" => {
                a.max_sum_error = Some(it.next().ok_or("--max-sum-error needs a number")?.parse().map_err(|e| format!("{e}"))?)
            }
            "--expect" => a.expect = Some(it.next().ok_or("--expect needs a path")?.into()),
            "--exact-residual" => a.exact_residual = Some(it.next().ok_or("--exact-residual needs a role")?),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => file = Some(PathBuf::from(s)),
        }
    }
    a.file = file.ok_or("usage: jksong-verify <file.jksong> [--strict] [--sum-reference raw.f32] [--expect expect.json]")?;
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let t0 = std::time::Instant::now();
    let rep = verify_file(&args.file, &VerifyOptions::default());
    println!("file: {}", rep.path.display());
    if let Some((ma, mi)) = rep.version {
        println!("version: {ma}.{mi}");
    }
    let mut failed = !rep.ok();
    if let Ok(song) = SongFile::open(&args.file) {
        let m = song.manifest();
        let meta = song.effective_metadata();
        println!(
            "song: \"{}\" by \"{}\" | {} Hz, {} ch, {} frames ({:.3} s) | preset {:?} | stems_sum_to_source {}",
            meta.title,
            meta.artist,
            m.sample_rate,
            m.channels,
            m.duration_frames,
            m.duration(),
            m.preset,
            m.stems_sum_to_source
        );
        let s = song.effective_settings();
        println!(
            "settings: semitones {} shift_drums {} guide_vocal {} backing_on {} backing_level {} lyrics_offset_ms {} levels {:?}",
            s.semitones, s.shift_drums, s.guide_vocal, s.backing_on, s.backing_level, s.lyrics_offset_ms, s.levels
        );
        if let Ok(Some(l)) = song.effective_lyrics() {
            for (i, line) in l.lines.iter().enumerate() {
                println!("lyrics line {i} [{:.3}..{:.3}]: {}", line.start, line.end, line.text());
            }
        }
    }
    for s in &rep.slots {
        println!("{s}");
    }
    for s in &rep.stems {
        println!(
            "stem {:<15} {:>9} bytes  gain {:<10.6} {} Hz {} ch {} bit  frames {} (decoded {:?})  md5 {:?}  seektable {:?} points, max gap {:?} s  seeks ok {:?} ({:?} frames compared)  peak {:?}",
            s.role,
            s.bytes,
            s.gain,
            s.sample_rate,
            s.channels,
            s.bits_per_sample,
            s.total_samples,
            s.decoded_frames,
            s.md5_ok,
            s.seek_points,
            s.max_seek_gap_seconds.map(|g| (g * 1000.0).round() / 1000.0),
            s.seeks_ok,
            s.seek_frames_compared,
            s.decoded_peak.map(|p| (p * 1e6).round() / 1e6)
        );
    }
    if let Some(g) = rep.lyrics_graphemes {
        println!("lyrics: {g} timed grapheme clusters");
    }

    if let Some(r) = &args.sum_reference {
        match sum_check(&args.file, r) {
            Ok((max_err, rms_err, bound, frames)) => {
                let exact = args.exact_residual.as_ref().and_then(|role| {
                    let song = SongFile::open(&args.file).ok()?;
                    let g = song.manifest().stem(role)?.gain;
                    Some(0.5 * g / 8_388_608.0 * (1.0 + 1e-9))
                });
                let limit = args.max_sum_error.or(exact).unwrap_or(bound);
                let pass = max_err <= limit;
                println!(
                    "sum of stems vs source: {frames} frames, max abs error {max_err:.3e} ({:.3} LSB of 24-bit), rms {rms_err:.3e}; limit {limit:.3e}: {}",
                    max_err * 8_388_608.0,
                    if pass { "PASS" } else { "FAIL" }
                );
                failed |= !pass;
            }
            Err(e) => {
                println!("ERROR sum check: {e}");
                failed = true;
            }
        }
    }
    if let Some(e) = &args.expect {
        match check_expect(&args.file, e) {
            Ok(n) => println!("expectations: {n} checked, all match"),
            Err(msg) => {
                println!("ERROR expectations: {msg}");
                failed = true;
            }
        }
    }

    for w in &rep.warnings {
        println!("WARNING {w}");
    }
    for e in &rep.errors {
        println!("ERROR {e}");
    }
    if args.strict && !rep.warnings.is_empty() {
        failed = true;
    }
    println!(
        "result: {} ({} errors, {} warnings, {:.2} s)",
        if failed { "FAIL" } else { "PASS" },
        rep.errors.len(),
        rep.warnings.len(),
        t0.elapsed().as_secs_f64()
    );
    if failed { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// Sums every stem at unity (integer / 8388608 * gain, in f64) and compares with the
/// reference. Returns (max abs error, rms error, bound, frames). The bound is the
/// 24-bit quantisation limit: half an LSB of each stem (times its gain), plus the
/// float32 rounding of the reference and of each float32 stem handed to the writer
/// (half an ulp at the largest magnitude involved).
fn sum_check(file: &PathBuf, reference: &PathBuf) -> Result<(f64, f64, f64, u64), String> {
    let song = SongFile::open(file).map_err(|e| e.to_string())?;
    let m = song.manifest();
    let ch = m.channels as usize;
    let raw = std::fs::read(reference).map_err(|e| e.to_string())?;
    let want: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    if want.len() != m.duration_frames as usize * ch {
        return Err(format!("reference has {} samples, song has {} x {ch}", want.len(), m.duration_frames));
    }
    let mut sum = vec![0f64; want.len()];
    let mut bound = 0f64;
    let mut peak_ref = want.iter().fold(0f32, |a, &b| a.max(b.abs())) as f64;
    for stem in &m.stems {
        let (_, rd) = song.stem_reader(&stem.role).map_err(|e| e.to_string())?;
        let mut dec = StemDecoder::new(rd, false).map_err(|e| e.to_string())?;
        let scale = stem.gain / 8_388_608.0;
        let mut block = Vec::new();
        let mut pos = 0usize;
        let mut stem_peak = 0f64;
        while let Some((_, n)) = dec.next_block(&mut block).map_err(|e| e.to_string())? {
            for (i, &s) in block.iter().enumerate() {
                let v = s as f64 * scale;
                stem_peak = stem_peak.max(v.abs());
                sum[pos + i] += v;
            }
            pos += n * ch;
        }
        if pos != sum.len() {
            return Err(format!("stem {} decoded {} samples, expected {}", stem.role, pos, sum.len()));
        }
        // Half an LSB at the stored scale, plus half a float32 ulp of the stem's peak.
        bound += 0.5 * scale + f32_half_ulp(stem_peak);
        peak_ref = peak_ref.max(stem_peak);
    }
    bound += f32_half_ulp(peak_ref);
    let mut max_err = 0f64;
    let mut sq = 0f64;
    for (s, w) in sum.iter().zip(&want) {
        let e = (s - *w as f64).abs();
        max_err = max_err.max(e);
        sq += e * e;
    }
    Ok((max_err, (sq / sum.len().max(1) as f64).sqrt(), bound, m.duration_frames))
}

/// Half the spacing of float32 values at magnitude `x`.
fn f32_half_ulp(x: f64) -> f64 {
    let x = (x as f32).abs().max(f32::MIN_POSITIVE);
    let next = f32::from_bits(x.to_bits() + 1);
    (next - x) as f64 / 2.0
}

/// Compares effective values with a JSON file:
/// `{"title": "...", "artist": "...", "album": null, "settings": {"semitones": 2, ...},
///   "lyrics_lines": ["text of line 0", ...], "lyrics_graphemes": [["c","a","f","é"], ...],
///   "user_state_keys": {"x_key": <value>}, "chunks": {"id": "<hex crc32>"}}`
/// Every key is optional. Returns how many values were checked.
fn check_expect(file: &PathBuf, expect: &PathBuf) -> Result<usize, String> {
    let song = SongFile::open(file).map_err(|e| e.to_string())?;
    let e: Value = serde_json::from_slice(&std::fs::read(expect).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut n = 0;
    let mut fail = Vec::new();
    let mut check = |what: String, got: Value, want: &Value| {
        n += 1;
        if &got != want {
            fail.push(format!("{what}: got {got}, expected {want}"));
        }
    };
    let meta = song.effective_metadata();
    for (k, got) in [("title", json!(meta.title)), ("artist", json!(meta.artist)), ("album", json!(meta.album))] {
        if let Some(w) = e.get(k) {
            check(k.into(), got, w);
        }
    }
    if let Some(Value::Object(s)) = e.get("settings") {
        let got = serde_json::to_value(song.effective_settings()).unwrap();
        for (k, w) in s {
            check(format!("settings.{k}"), got.get(k).cloned().unwrap_or(Value::Null), w);
        }
    }
    let lyr = song.effective_lyrics().map_err(|e| e.to_string())?;
    if let Some(Value::Array(lines)) = e.get("lyrics_lines") {
        let got: Vec<String> = lyr.as_ref().map(|l| l.lines.iter().map(|x| x.text()).collect()).unwrap_or_default();
        check("lyrics_lines".into(), json!(got), &Value::Array(lines.clone()));
    }
    if let Some(w) = e.get("lyrics_graphemes") {
        let got: Vec<Vec<String>> = lyr
            .as_ref()
            .map(|l| l.lines.iter().flat_map(|x| &x.words).map(|w| w.chars.iter().map(|c| c.text.clone()).collect()).collect())
            .unwrap_or_default();
        check("lyrics_graphemes".into(), json!(got), w);
    }
    if let Some(Value::Object(keys)) = e.get("user_state_keys") {
        let st = serde_json::to_value(song.user_state()).unwrap();
        for (k, w) in keys {
            check(format!("user_state.{k}"), st.get(k).cloned().unwrap_or(Value::Null), w);
        }
    }
    if let Some(Value::Object(chunks)) = e.get("chunks") {
        for (id, w) in chunks {
            let got = song.chunk(id).map(|c| json!(format!("{:08x}", c.crc32))).unwrap_or(Value::Null);
            check(format!("chunk {id} crc32"), got, w);
        }
    }
    if fail.is_empty() { Ok(n) } else { Err(fail.join("; ")) }
}
