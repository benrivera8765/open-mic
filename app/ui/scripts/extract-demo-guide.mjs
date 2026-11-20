// SPDX-License-Identifier: GPL-3.0-or-later
// Copies the melody guide (docs/SONG_FORMAT.md section 10) and the lyrics of a .jksong
// into src/practice/fixtures/demo-song.practice.json, the ground truth of the practice
// lane's whole-song checks (src/practice/lane.test.ts) and the browser mock's demo guide.
//
//   node scripts/extract-demo-guide.mjs <song-with-guide.jksong> [out.json]
//
// The input is a COPY of tests/fixtures/generated/demo-song.jksong (public-domain
// "Twinkle, Twinkle, Little Star", all audio synthesised) to which
// `jukeoke-cli guide add` has added its guide; the repository's own demo song has no
// guide. Reads the header, the table of contents, the manifest, `guide`, `guide.curve`
// and the lyrics chunk, checking every CRC. The curve is kept with two decimals
// (about 1 cent), as `practice_guide` sends it (docs/APP_API.md 12.5).
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const input = process.argv[2];
if (!input) {
  console.error("usage: node scripts/extract-demo-guide.mjs <song-with-guide.jksong> [out.json]");
  process.exit(2);
}
const output = process.argv[3] ?? join(here, "..", "src", "practice", "fixtures", "demo-song.practice.json");

const table = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (b) => {
  let c = 0xffffffff;
  for (const x of b) c = table[(c ^ x) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

const buf = readFileSync(input);
const bytes = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const magic = [0x89, 0x4a, 0x4b, 0x53, 0x0d, 0x0a, 0x1a, 0x0a];
if (!magic.every((b, i) => bytes[i] === b)) throw new Error(`${input} is not a .jksong file`);
if (crc32(bytes.subarray(0, 124)) !== dv.getUint32(124, true)) throw new Error("header CRC mismatch");
const tocOff = Number(dv.getBigUint64(16, true));
const toc = bytes.subarray(tocOff, tocOff + Number(dv.getBigUint64(24, true)));
if (crc32(toc) !== dv.getUint32(32, true)) throw new Error("table of contents CRC mismatch");
const chunks = JSON.parse(new TextDecoder().decode(toc)).chunks;
const raw = (id) => {
  const c = chunks.find((k) => k.id === id);
  if (!c) throw new Error(`chunk ${id} missing`);
  const data = bytes.subarray(c.offset, c.offset + c.length);
  if (crc32(data) !== c.crc32) throw new Error(`chunk ${id}: CRC mismatch`);
  return data;
};
const json = (id) => JSON.parse(new TextDecoder().decode(raw(id)));

const manifest = json("manifest");
const lyrics = json(manifest.lyrics);
const guide = json("guide");

// guide.curve (SONG_FORMAT 10.3): header, field table, planar f32 fields.
const cb = raw(guide.curve ?? "guide.curve");
const cv = new DataView(cb.buffer, cb.byteOffset, cb.byteLength);
if (String.fromCharCode(cb[0], cb[1], cb[2], cb[3]) !== "JKPC") throw new Error("guide.curve: bad magic");
const fields = cv.getUint16(6, true);
const frames = cv.getUint32(8, true);
const hopUs = cv.getUint32(12, true);
const t0Us = cv.getInt32(16, true);
let midiAt = -1;
for (let f = 0; f < fields; f++) {
  const tag = String.fromCharCode(...cb.subarray(32 + 8 * f, 36 + 8 * f));
  if (tag === "MIDI" && cv.getUint32(36 + 8 * f, true) === 1) midiAt = f;
}
if (midiAt < 0) throw new Error("guide.curve: no MIDI field");
const base = 32 + 8 * fields + 4 * frames * midiAt;
const midi = [];
for (let k = 0; k < frames; k++) midi.push(Math.round(cv.getFloat32(base + 4 * k, true) * 100) / 100);

const out = {
  source: "tests/fixtures/generated/demo-song.jksong + jukeoke-cli guide add (public-domain melody and words, synthesised audio)",
  title: manifest.title,
  duration_s: manifest.duration_seconds,
  key: manifest.analysis?.key ?? null,
  lyrics,
  guide: {
    guide_version: guide.guide_version,
    low_confidence: guide.low_confidence,
    tuning: guide.tuning,
    range: guide.range,
    counts: guide.counts,
    notes: guide.notes,
    spans: guide.spans ?? [],
    words: guide.words ?? null,
    provenance: { method: guide.provenance?.method, version: guide.provenance?.version, created_utc: guide.provenance?.created_utc },
  },
  curve: { hop_s: hopUs / 1e6, t0_s: t0Us / 1e6, midi },
};
writeFileSync(output, JSON.stringify(out) + "\n");
console.log(`${output}: ${guide.notes.length} notes, ${frames} curve frames, tuning ${guide.tuning?.use_cents} cents`);
