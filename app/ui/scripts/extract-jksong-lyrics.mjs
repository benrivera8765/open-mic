// SPDX-License-Identifier: GPL-3.0-or-later
// Copies the lyrics of a .jksong into src/api/demo-song.lyrics.json, where the browser
// mock picks them up for its Twinkle demo song (src/api/demo.ts), so the mock shows
// exactly the lyrics of the engine's generated demo song.
//
//   node scripts/extract-jksong-lyrics.mjs [song.jksong]
//
// Default input: tests/fixtures/generated/demo-song.jksong. Reads the header, the
// table of contents and the manifest (docs/SONG_FORMAT.md sections 2-4) and checks
// their CRCs. Delete the JSON file to go back to the generated demo lyrics.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const input = process.argv[2] ?? join(here, "..", "..", "..", "tests", "fixtures", "generated", "demo-song.jksong");
const output = join(here, "..", "src", "api", "demo-song.lyrics.json");

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
const read = (id) => {
  const c = chunks.find((k) => k.id === id);
  if (!c) throw new Error(`chunk ${id} missing`);
  const data = bytes.subarray(c.offset, c.offset + c.length);
  if (crc32(data) !== c.crc32) throw new Error(`chunk ${id} CRC mismatch`);
  return JSON.parse(new TextDecoder().decode(data));
};
const manifest = read("manifest");
if (!manifest.lyrics) throw new Error(`${input} has no lyrics`);
const lyrics = read(manifest.lyrics);
writeFileSync(output, `${JSON.stringify(lyrics, null, 1)}\n`);
console.log(`wrote ${output}: ${lyrics.lines.length} lines from "${manifest.title}"`);
