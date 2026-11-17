# The `.jksong` song file, version 1.0

A `.jksong` file holds everything needed to sing one song: lossless stems, lyrics
timed to the letter, metadata, the singer's saved settings, and provenance. It is a
single file, so it survives USB sticks, AirDrop, cloud sync and email.

This document is the specification. The reference implementations are
`crates/jk-format` (Rust reader and writer) and `studio/jukeoke_studio/jksong.py`
(Python writer and reader). They are tested against each other. Anyone may write
tools for this format.

Conventions: all integers are unsigned little-endian. Offsets are absolute byte
offsets from the start of the file. "CRC-32" is CRC-32/ISO-HDLC (the zlib / PNG /
Ethernet CRC, polynomial 0x04C11DB7 reflected, init and xorout 0xFFFFFFFF). JSON is
UTF-8 without a byte-order mark. Times are seconds as JSON numbers.

## 1. Layout

```
offset 0                 file header (128 defined bytes, zero-padded to 4096)
offset 4096              settings slot A   (slot_size bytes)
offset 4096 + slot_size  settings slot B   (slot_size bytes)
...                      chunks, each starting on a 4096-byte boundary, zero padding between
toc_offset               table of contents (JSON), normally the last thing in the file
```

The design goal of the two slots: the singer's settings (key, mix, timing offset,
lyric corrections) change often and must travel with the file, but rewriting a
100 MB file on every change is slow and unsafe on a USB stick. The slots are updated
in place, alternately, each with a sequence number and a checksum, so an interrupted
write can only damage the slot being written, and the other slot still holds the
previous good state.

## 2. File header

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 8 | magic | `89 4A 4B 53 0D 0A 1A 0A` (`\x89JKS\r\n\x1A\n`, PNG-style: detects text-mode and line-ending damage) |
| 8 | 2 | major | `1` |
| 10 | 2 | minor | `0` |
| 12 | 4 | header_length | `128` |
| 16 | 8 | toc_offset | offset of the table of contents |
| 24 | 8 | toc_length | its length in bytes |
| 32 | 4 | toc_crc32 | CRC-32 of the TOC bytes |
| 36 | 4 | slot_size | bytes per settings slot; writers use 262144 (256 KiB); minimum 4096; multiple of 4096 |
| 40 | 8 | slot_a_offset | `4096` |
| 48 | 8 | slot_b_offset | `4096 + slot_size` |
| 56 | 8 | file_length | total file length in bytes (detects truncation) |
| 64 | 60 | reserved | zero; readers ignore |
| 124 | 4 | header_crc32 | CRC-32 of bytes 0..124 |

Bytes 128..4096 are zero. Readers must:

1. Check the magic. If it is wrong, the file is not a song file.
2. Check `header_crc32`. If it is wrong, the file is damaged.
3. If `major` is greater than the reader supports, refuse with "this song was made by
   a newer version; please update". Any `minor` is accepted.
4. If the real file length is smaller than `file_length`, report the file as truncated
   (an incomplete copy) and refuse to play it.

## 3. Table of contents

UTF-8 JSON at `toc_offset`, checked with `toc_crc32`:

```json
{
  "chunks": [
    { "id": "manifest",        "type": "application/json", "offset": 528384,  "length": 2911,     "crc32": 1234567 },
    { "id": "lyrics",          "type": "application/json", "offset": 532480,  "length": 48210,    "crc32": 7654321 },
    { "id": "cover",           "type": "image/jpeg",       "offset": 581632,  "length": 183022,   "crc32": 1111111 },
    { "id": "stem.lead_vocal", "type": "audio/flac",       "offset": 765952,  "length": 21004311, "crc32": 2222222 }
  ]
}
```

- `id` is unique in the file. Readers ignore chunk ids they do not know.
- `crc32` covers the chunk's bytes. Players need not check large chunks on every
  open (FLAC frames carry their own checksums); `jukeoke-cli verify` checks them all.
- A tool that rewrites a file must copy chunks it does not understand unchanged.

Required chunks: `manifest`, and every chunk the manifest refers to.

## 4. Manifest (`manifest` chunk)

```json
{
  "format": "jksong",
  "format_version": "1.0",
  "title": "Song title",
  "artist": "Artist",
  "album": null,
  "sample_rate": 44100,
  "channels": 2,
  "duration_frames": 10584000,
  "duration_seconds": 240.0,
  "cover": "cover",
  "lyrics": "lyrics",
  "analysis": {
    "key": { "tonic": "A", "mode": "major", "name": "A major", "confidence": 0.81 },
    "tempo_bpm": 118.9
  },
  "preset": "full",
  "stems_sum_to_source": true,
  "stems": [
    { "role": "lead_vocal",     "chunk": "stem.lead_vocal",     "codec": "flac", "bits": 24, "gain": 1.0,    "peak": 0.71, "in_karaoke_mix": false, "pitched": true },
    { "role": "backing_vocals", "chunk": "stem.backing_vocals", "codec": "flac", "bits": 24, "gain": 1.0,    "peak": 0.33, "in_karaoke_mix": true,  "pitched": true },
    { "role": "drums",          "chunk": "stem.drums",          "codec": "flac", "bits": 24, "gain": 1.0,    "peak": 0.95, "in_karaoke_mix": true,  "pitched": false },
    { "role": "other",          "chunk": "stem.other",          "codec": "flac", "bits": 24, "gain": 1.0417, "peak": 1.03, "in_karaoke_mix": true,  "pitched": true }
  ],
  "provenance": { "...": "see section 4.3" }
}
```

### 4.1 Fields

- `title`, `artist` (strings), `album` (string or null). The Studio lets the user correct them.
- `sample_rate`, `channels`: of every stem. `channels` is 1 or 2.
- `duration_frames`: length of every stem in sample frames. `duration_seconds` is
  informational (`duration_frames / sample_rate`).
- `cover`: chunk id of the cover image (JPEG or PNG, at most 2 MB), or null.
- `lyrics`: chunk id of the lyrics (section 5), or null for an instrumental.
- `analysis.key`: `tonic` is one of `C C# D D# E F F# G G# A A# B` (sharps only;
  players may display flats), `mode` is `major` or `minor`, `confidence` 0..1. Null if
  unknown. `tempo_bpm` is a number or null.
- `preset`: which Studio quality preset made the stems (`full` or `clean`, see
  `docs/benchmarks/separation.md`).
- `stems_sum_to_source`: true when all stems at unity sum back to the source
  (the `full` preset).

### 4.2 Stems

Each stem is a complete FLAC stream (starting with `fLaC`) in its own chunk:

- 24 bits per sample, `channels` channels, `sample_rate` Hz, exactly
  `duration_frames` frames. STREAMINFO carries the total sample count and the MD5.
- A SEEKTABLE with a seek point at least every 10 seconds is recommended. Players
  must seek sample-accurately with or without it.
- **Gain.** The true float sample is `decoded_integer / 8388608 * gain`. Writers keep
  `gain = 1.0` unless the stem's float peak would clip; then they scale the stem down
  so its peak is at most 0.99 and store `gain = 1 / scale`. Separated stems can exceed
  full scale; this is how they are stored without clipping. `peak` is the float peak
  before scaling (informational).
- `role`: `lead_vocal`, `backing_vocals`, `drums`, `other` in version 1.0. Future
  versions may add roles (for example `bass`).
- `in_karaoke_mix`: whether the stem is in the default karaoke mix.
- `pitched`: whether the stem goes through the key change by default (drums do not).
- **Forward compatibility rule:** a player that meets a `role` it does not know plays
  that stem at unity gain when `in_karaoke_mix` is true, through the pitch shifter
  when `pitched` is true. That way a newer file with more stems still sounds right
  in an older player.

The default karaoke mix is every stem with `in_karaoke_mix: true`. The guide vocal is
the `lead_vocal` stem mixed in at the singer's chosen level.

### 4.3 Provenance

```json
"provenance": {
  "app":    { "name": "Jukeoke", "version": "0.1.0" },
  "studio": { "version": "0.1.0", "python": "3.12.16", "torch": "2.14.1+cu130", "device": "NVIDIA GeForce RTX 5070 Ti" },
  "source": { "filename": "original.mp3", "sha256": "...", "container": "mp3", "codec": "mp3",
              "sample_rate": 44100, "channels": 2, "bits_per_sample": null, "lossy": true },
  "models": [
    { "task": "vocals", "name": "...", "file": "...", "sha256": "...", "licence": "...", "source_url": "..." }
  ],
  "resampling": [
    { "what": "model input and its vocal estimate", "from_hz": 48000, "to_hz": 44100, "method": "soxr VHQ" }
  ],
  "created_utc": "2026-11-17T09:30:00Z"
}
```

`resampling` lists every resample that touched any stem (empty when none). The
original audio itself is never resampled by the Studio.

## 5. Lyrics (`lyrics` chunk)

```json
{
  "lyrics_version": 1,
  "language": "en",
  "source": "transcribed",
  "checked": false,
  "alignment": { "method": "ctc-forced", "model": "...", "anchored_lines": true },
  "lines": [
    {
      "start": 12.340, "end": 15.610,
      "words": [
        { "start": 12.340, "end": 12.900, "confidence": 0.93,
          "chars": [["H", 12.340, 12.400], ["e", 12.400, 12.470], ["l", 12.470, 12.540],
                    ["l", 12.540, 12.610], ["o", 12.610, 12.900]] },
        { "start": 13.020, "end": 13.800, "confidence": 0.88,
          "chars": [["w", 13.020, 13.110], ["o", 13.110, 13.200], ["r", 13.200, 13.400],
                    ["l", 13.400, 13.600], ["d", 13.600, 13.700], [",", 13.700, 13.800]] }
      ]
    }
  ]
}
```

- `source`: `user_text`, `lrc`, `embedded`, `online:<service>` (for example
  `online:lrclib`), or `transcribed`.
- `checked`: false until a person has reviewed the text. Transcribed lyrics start
  unchecked and the app labels them so.
- Each word's text is the concatenation of its `chars`. A line's text is its words'
  texts joined with single spaces. Punctuation stays attached to its word.
- `chars` are **grapheme clusters** (Unicode extended grapheme clusters, UAX #29),
  stored explicitly as strings so that implementations never have to agree on
  segmentation: `[grapheme, start, end]`.
- Times: `start <= end`; within a word, chars are in order and do not overlap; words
  within a line and lines within the song are in order and do not overlap. A held
  note shows up as a long final char (for example the `o` above).
- `confidence` (0..1) is optional.
- Lines may carry an optional `"kind": "backing"` in later versions; readers ignore
  unknown keys everywhere.

## 6. Settings slots

Each slot is `slot_size` bytes:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `JKSL` |
| 4 | 4 | slot version `1` |
| 8 | 8 | sequence number (0 = never written) |
| 16 | 4 | payload length in bytes |
| 20 | 4 | encoding: `0` UTF-8 JSON, `1` zlib-compressed (RFC 1950) UTF-8 JSON |
| 24 | 4 | reserved, zero |
| 28 | 4 | CRC-32 of bytes 0..28 followed by the payload |
| 32 | ... | payload, then zero padding to `slot_size` |

A slot is **valid** when the magic, version and CRC are right and the payload fits.
The **active** slot is the valid slot with the higher sequence number. If neither is
valid, there are no saved settings (use defaults).

To save, a writer reads the active slot, merges its changes into the JSON (keeping
keys it does not understand), writes the result into the *other* slot with sequence
number = active + 1, and flushes it to disk. It never touches the active slot. If
the payload does not fit in a slot, the writer rewrites the whole file instead
(section 7).

Payload (the "user state"):

```json
{
  "user_state_version": 1,
  "updated_utc": "2026-11-17T21:05:00Z",
  "settings": {
    "semitones": 2,
    "shift_drums": false,
    "guide_vocal": 0.0,
    "backing_on": true,
    "backing_level": 1.0,
    "levels": { "drums": 1.0, "other": 1.0 },
    "lyrics_offset_ms": 0
  },
  "metadata_override": { "title": "Corrected title" },
  "lyrics_override": null
}
```

- `semitones`: integer key shift, -12..+12. Saved with the song so the chosen key
  travels to the Mac.
- `lyrics_offset_ms`: per-song timing offset (positive = lyrics later). Device
  latency compensation for Bluetooth is a per-device app setting, not stored here.
- `lyrics_override`: when the singer corrects timing or text in the sync editor, the
  corrected lyrics (same schema as section 5) are stored here and win over the
  `lyrics` chunk.
- `metadata_override`: corrected title/artist/album; wins over the manifest.

## 7. Writing a whole file

Writers create `<name>.jksong.tmp-<random>` in the destination folder, write the
complete file, flush it to disk, then rename it over the destination. A crash or a
pulled stick leaves either the old file or the new file, never a half-written one.
Readers ignore `*.tmp-*` files, dotfiles and macOS `._*` files.

Write order: header placeholder, both slots (the initial user state in slot A with
sequence 1, slot B empty), chunks (each 4096-aligned), TOC, then the final header.

## 8. Versioning

- New optional chunks, new JSON keys and new stem roles are **minor** changes:
  older readers ignore what they do not know, so newer files still open in older players.
- A change that older readers would misinterpret is a **major** change. Readers
  refuse files with a higher major version and say why.

## 9. Validation checklist (`jukeoke-cli verify`)

Magic, header CRC, file length, TOC CRC, every chunk CRC and bounds, manifest
schema, every referenced chunk present, every stem a valid FLAC stream with the
declared rate, channels, bit depth and length, lyrics times ordered and within the
song, each word's chars non-empty, slots valid or empty.
