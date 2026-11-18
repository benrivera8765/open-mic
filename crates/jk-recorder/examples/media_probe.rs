// SPDX-License-Identifier: GPL-3.0-or-later
//! Lists the media players Windows knows about (read-only: it never plays, pauses
//! or changes anything). Shows whether the WinRT link works on this build.
//!
//!   cargo run -p jk-recorder --example media_probe [--titles] [--watch <id> <seconds>]
//!
//! Without `--titles` the track names are not printed, only whether there are any.

use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let titles = args.iter().any(|a| a == "--titles");
    match jk_recorder::media::available() {
        Ok(()) => println!("media link: available"),
        Err(e) => {
            println!("media link: not available: {e}");
            return;
        }
    }
    let t0 = Instant::now();
    match jk_recorder::media::sessions() {
        Ok(list) => {
            println!("{} session(s), listed in {:.1} ms", list.len(), t0.elapsed().as_secs_f64() * 1000.0);
            for s in list {
                let t = &s.track;
                let what = if titles {
                    format!("{:?} / {:?} / {:?}", t.artist, t.title, t.album)
                } else {
                    format!("title {}, artist {}", if t.title.is_empty() { "none" } else { "present" }, if t.artist.is_empty() { "none" } else { "present" })
                };
                println!(
                    "  id {:?} app {:?} ({}) current={} status={} play={} pause={} duration={:?} position={:?} | {what}",
                    s.id, s.source_app_id, s.app_name, s.is_current, s.status.as_str(), s.can_play, s.can_pause, t.duration_s, t.position_s
                );
            }
        }
        Err(e) => println!("listing failed: {e}"),
    }
    if let Some(i) = args.iter().position(|a| a == "--watch") {
        let id = args.get(i + 1).cloned().unwrap_or_default();
        let secs: f64 = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
        let t0 = Instant::now();
        let w = jk_recorder::media::MediaWatch::start(&id, move |o| {
            println!(
                "  +{:8.3} s {:?} status={} title={:?} artist={:?} duration={:?} position={:?} gone={}",
                o.at.duration_since(t0).as_secs_f64(),
                o.cause,
                o.status.as_str(),
                if titles { o.track.title.clone() } else { (!o.track.title.is_empty()).to_string() },
                if titles { o.track.artist.clone() } else { (!o.track.artist.is_empty()).to_string() },
                o.track.duration_s,
                o.track.position_s,
                o.gone
            );
        });
        match w {
            Ok(w) => {
                std::thread::sleep(Duration::from_secs_f64(secs));
                drop(w);
            }
            Err(e) => println!("watch failed: {e}"),
        }
    }
}
