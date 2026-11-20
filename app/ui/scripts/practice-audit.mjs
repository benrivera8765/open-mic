// SPDX-License-Identifier: GPL-3.0-or-later
// File audit around a practice session (WO2 section 9 "Privacy and portability"): every
// file created or changed since <since> under the portable folder and the per-user folders
// (LocalAppData, AppData, Temp, Documents, Music), with any audio file flagged. Microphone
// sound must never be written, and nothing may be written outside the portable folder.
//
//   node scripts/practice-audit.mjs <since ISO time> <portable root> <out.json>
//
// Only names, sizes and times are listed; no file is read.
import { readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [sinceArg, root, out] = process.argv.slice(2);
if (!sinceArg || !root || !out) {
  console.error("usage: node scripts/practice-audit.mjs <since ISO> <portable root> <out.json>");
  process.exit(2);
}
const since = Date.parse(sinceArg);
const home = process.env.USERPROFILE ?? "";
const roots = {
  portable: root,
  LocalAppData: process.env.LOCALAPPDATA,
  AppData: process.env.APPDATA,
  Temp: process.env.TEMP,
  Documents: join(home, "Documents"),
  Music: join(home, "Music"),
};
const AUDIO = /\.(wav|wave|w64|rf64|raw|pcm|flac|mp3|m4a|aac|ogg|opus|aif|aiff|caf)$/i;
const report = { since: new Date(since).toISOString(), roots: {}, audio_files: [] };
for (const [name, dir] of Object.entries(roots)) {
  if (!dir) continue;
  const changed = [];
  let scanned = 0;
  const walk = (d, depth) => {
    let entries;
    try {
      entries = readdirSync(d, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = join(d, e.name);
      if (e.isSymbolicLink()) continue;
      if (e.isDirectory()) {
        if (depth < 14) walk(p, depth + 1);
        continue;
      }
      scanned++;
      let st;
      try {
        st = statSync(p);
      } catch {
        continue;
      }
      const t = Math.max(st.mtimeMs, st.birthtimeMs);
      if (t >= since) {
        changed.push({ path: p, bytes: st.size, mtime: new Date(st.mtimeMs).toISOString(), created: new Date(st.birthtimeMs).toISOString() });
        if (AUDIO.test(p)) report.audio_files.push({ root: name, path: p, bytes: st.size });
      }
    }
  };
  walk(dir, 0);
  report.roots[name] = { dir, scanned, changed };
  console.log(`${name}: ${changed.length} files changed since ${report.since} (of ${scanned} scanned)`);
}
writeFileSync(out, JSON.stringify(report, null, 2));
console.log(`audio files created or changed: ${report.audio_files.length}`);
