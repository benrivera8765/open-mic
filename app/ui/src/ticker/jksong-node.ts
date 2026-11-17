// SPDX-License-Identifier: GPL-3.0-or-later
// Minimal .jksong reader for Node-side tests and scripts: header -> table of contents
// -> manifest -> lyrics chunk (docs/SONG_FORMAT.md sections 2-5). Checks the magic
// and the CRCs of the parts it reads. Never shipped in the app (the UI never opens
// song files; the Rust core does).
import { readFileSync } from "node:fs";
import type { Lyrics } from "../api/types";

const MAGIC = [0x89, 0x4a, 0x4b, 0x53, 0x0d, 0x0a, 0x1a, 0x0a];

let crcTable: Uint32Array | null = null;
/** CRC-32/ISO-HDLC (zlib). */
export function crc32(bytes: Uint8Array): number {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c >>> 0;
    }
  }
  let c = 0xffffffff;
  for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

export interface JksongContents {
  manifest: Record<string, unknown> & { title?: string; duration_seconds?: number; lyrics?: string | null };
  lyrics: Lyrics | null;
}

/** Reads the manifest and the lyrics of a .jksong file. Throws with a plain message on damage. */
export function readJksong(path: string): JksongContents {
  const buf = readFileSync(path);
  const bytes = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
  if (!MAGIC.every((b, i) => bytes[i] === b)) throw new Error(`${path}: not a .jksong file (bad magic)`);
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (crc32(bytes.subarray(0, 124)) !== dv.getUint32(124, true)) throw new Error(`${path}: header CRC mismatch`);
  const tocOffset = Number(dv.getBigUint64(16, true));
  const tocLength = Number(dv.getBigUint64(24, true));
  const toc = bytes.subarray(tocOffset, tocOffset + tocLength);
  if (crc32(toc) !== dv.getUint32(32, true)) throw new Error(`${path}: table of contents CRC mismatch`);
  const chunks = (JSON.parse(new TextDecoder().decode(toc)) as { chunks: { id: string; offset: number; length: number; crc32: number }[] }).chunks;
  const chunk = (id: string) => {
    const c = chunks.find((k) => k.id === id);
    if (!c) throw new Error(`${path}: chunk ${id} missing`);
    const data = bytes.subarray(c.offset, c.offset + c.length);
    if (crc32(data) !== c.crc32) throw new Error(`${path}: chunk ${id} CRC mismatch`);
    return JSON.parse(new TextDecoder().decode(data)) as unknown;
  };
  const manifest = chunk("manifest") as JksongContents["manifest"];
  const lyrics = typeof manifest.lyrics === "string" ? (chunk(manifest.lyrics) as Lyrics) : null;
  return { manifest, lyrics };
}
