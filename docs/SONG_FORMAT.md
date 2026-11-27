# The `.jksong` song file, version 1.1

A `.jksong` file holds everything needed to sing one song: lossless stems, lyrics
timed to the letter, metadata, the singer's saved settings, and provenance. It is a
single file, so it survives USB sticks, AirDrop, cloud sync and email.

Version 1.1 (Work Order 2, practice mode) adds optional data under the minor-change
rules of section 8: the **melody guide** (section 10: chunks `guide` and
`guide.curve`) and the singer's **practice data** in the settings slot (section 11:
key `practice`). A 1.0 reader ignores both and still opens and plays the file; section
12 lists every change.

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
| 10 | 2 | minor | `1` (written by 1.1 writers; 1.0 writers wrote `0`; any minor is accepted) |
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

Optional chunks with well-known ids (version 1.1): `guide` and `guide.curve`, the
melody guide (section 10). They are found by their ids in the TOC, not through the
manifest, so that adding a guide to an existing file leaves the manifest byte for byte
as it was (section 7.1).

## 4. Manifest (`manifest` chunk)

```json
{
  "format": "jksong",
  "format_version": "1.1",
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

- `format_version`: the version of this specification the writer of the manifest
  followed (`"1.1"` for new files from 1.1 writers; files written before are `"1.0"`).
  Readers refuse a higher **major** part and accept any minor. A minor version only
  adds optional data, so readers never use `format_version` to decide what a file
  holds: they look for the chunks and keys. A guide added to a 1.0 file later leaves
  its manifest (and so its `"1.0"`) unchanged (section 7.1).
- `title`, `artist` (strings), `album` (string or null). The Studio lets the user correct them.
- `sample_rate`, `channels`: of every stem. `channels` is 1 or 2.
- `duration_frames`: length of every stem in sample frames. `duration_seconds` is
  informational (`duration_frames / sample_rate`).
- `cover`: chunk id of the cover image (JPEG or PNG, at most 2 MB), or null.
- `lyrics`: chunk id of the lyrics (section 5), or null for an instrumental.
- `analysis.key`: `tonic` is one of `C C# D D# E F F# G G# A A# B` (sharps only;
  players may display flats), `mode` is `major` or `minor`, `confidence` 0..1. Null if
  unknown. `tempo_bpm` is a number or null.
- `preset`: which Studio quality preset made the stems (`full`, `clean` or
  `permissive`, see `docs/benchmarks/separation.md` and `MODELS.md`; `permissive`
  uses MIT / Apache-2.0 models only and leaves `backing_vocals` silent). Readers accept
  any string (a newer Studio may add presets); `jukeoke-cli verify` warns on others.
- `stems_sum_to_source`: true when all stems at unity sum back to the source (every
  preset of the current Studio: `other` is computed as the residual).

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
- Backing vocals (DECISIONS D-282): words written in parentheses, `(` `)` or `（` `）`
  ("(Can't help myself) I get so lonely"), and the words of a line with
  `"kind": "backing"`, are the backing singers', not the lead's. Readers show them as
  secondary and give them no practice bars or grade. The exact rule is
  `jk_format::Lyrics::backing_words`; nothing is stored for it.

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
- `practice` **(1.1)**: the singer's practice settings and results for this song
  (section 11). Absent until the song is first practised.

## 7. Writing a whole file

Writers create `<name>.jksong.tmp-<random>` in the destination folder, write the
complete file, flush it to disk, then rename it over the destination. A crash or a
pulled stick leaves either the old file or the new file, never a half-written one.
Readers ignore `*.tmp-*` files, dotfiles and macOS `._*` files.

Write order: header placeholder, both slots (the initial user state in slot A with
sequence 1, slot B empty), chunks (each 4096-aligned), TOC, then the final header.

### 7.1 Adding or replacing the guide in an existing file (1.1)

Adding a guide to a song that already exists (WO2 4.3: "it must not change one byte of
the stems, lyrics, cover or saved settings") is a whole-file rewrite by the rule above,
with these extra rules. Implementations: a raw-copy mode of `jk_format::rewrite_song`
(Rust; the app core and `jukeoke-cli guide add`) and the Python writer (conformance
tests). The 1.0 `rewrite_song` is not enough as it is: it re-encodes the manifest and
writes the user state into slot A only (its job is to grow the slots, section 6), so
Stage 2 adds the mode that copies the manifest chunk and both slot regions raw.

1. **Build first, from the file itself.** The guide is made from the file's
   `lead_vocal` stem (section 10.5); word references come from the effective lyrics. No
   separation is run again. The song may keep playing meanwhile (the file is only read).
2. **Write the temporary file** `<name>.jksong.tmp-<random>` next to the song:
   - the header placeholder;
   - **both settings slots copied raw**: the exact `slot_size` bytes of slot A and of
     slot B, sequence numbers, CRCs and padding included, with the same `slot_size`.
     The user state is neither parsed nor merged nor re-encoded, so the saved settings
     are unchanged to the byte, even a slot that is invalid or never written;
   - **every existing chunk, in TOC order, copied byte for byte**, its CRC-32 checked
     while copying (a mismatch stops the rewrite: the file is damaged and is left
     alone). This includes the `manifest` (never touched: the guide is found by its
     chunk id, section 3), every stem, `lyrics`, `cover` and chunks this writer does
     not know. TOC entries keep their `id`, `type`, `length`, `crc32` and any unknown
     keys; unknown top-level TOC keys are kept;
   - `guide` and `guide.curve`: an existing guide is replaced in its place in the
     chunk order; a new one is appended after the last chunk;
   - the TOC, then the final header (`minor` 1, new `toc_*`, `file_length`).
3. **Copy the slots again, last.** Immediately before the rename, while the writer
   holds the song's write lock (no settings save can run; in the app the song is
   released first, step 5), slot A and slot B are copied raw once more from the
   original file into the temporary one, so a settings save made while step 2 was
   writing (a key change, a practice result) is not lost. Then the temporary file is
   flushed.
4. **Check before replacing.** The temporary file is opened as a song: header and TOC
   CRCs valid; every chunk except `guide` and `guide.curve` has the same `id`, `type`,
   `length` and `crc32` as in the original's TOC; the new guide chunks' CRCs match;
   the slot regions are byte-identical to the original's. "The original" is the file as
   it is at this moment (header and TOC read again under the lock), not as it was when
   step 2 began: the swap may wait a long time (a playing song, docs/APP_API.md 12.3), and
   meanwhile a whole-file rewrite of the original (a `lyrics_override` that outgrew its
   slot, section 6) may have changed its slot size or manifest. Any difference: the
   temporary file is deleted, the original stays, and the guide is built again from it.
5. **Replace.** Rename over the original (atomic; on Unix the folder is flushed after).
   On Windows, and on exFAT/FAT everywhere, a file that is open cannot be replaced:
   the app first releases a loaded song exactly as the delete feature does (stop,
   unload, flush the pending slot save, wait until no handle is open: D-231, D-233),
   renames, then loads the song again at the same position.
6. **Interrupted.** Before the rename the original is untouched and a stray
   `*.tmp-*` file is left (readers ignore it; the guide builder deletes its own leftovers
   older than one hour); after it, the new file is complete. A pulled stick therefore
   leaves the old file or the new one.

What changes in the file: the header (TOC position, length and CRC, `file_length`,
`minor`), the TOC (two entries more) and the bytes of the guide chunks. Everything else
is the same bytes, and in practice at the same offsets (old chunks keep their order and
4096-byte alignment; the guide is appended). A check for "nothing else changed" compares
the SHA-256 of every other chunk and of both slot regions before and after.

The rewrite needs free space for one more copy of the song (about 100-150 MB). When the
file cannot be written (read-only medium, no space, file in use), the original stays as
it is and the reason is reported; the app may still use a guide built in memory for that
session (docs/APP_API.md section 12.3).

## 8. Versioning

- New optional chunks, new JSON keys and new stem roles are **minor** changes:
  older readers ignore what they do not know, so newer files still open in older players.
- A change that older readers would misinterpret is a **major** change. Readers
  refuse files with a higher major version and say why.
- Version 1.1 is a minor change (section 12). A 1.0 reader ignores the `guide` and
  `guide.curve` chunks (unknown chunk ids) and the `practice` key (unknown key); it
  keeps both when it writes: a slot save merges into the active state keeping unknown
  keys (section 6), and a whole-file rewrite copies unknown chunks unchanged (section 3).
  The 1.0 `jukeoke-cli verify` reports no warning for either. Checked 2026-11-19 with
  the pre-WO2 tools on the generated demo song laid out as 1.1 (header minor 1,
  `format_version` "1.1", both guide chunks, a `practice` key; `compat_probe.py` and its
  results under `$JK_BUILD/practice-contracts/compat/`): `jukeoke-cli verify --strict`
  PASS with 0 errors and 0 warnings; its renders at keys 0 and +2 are bit-identical to the
  original file's; the 1.0 Python slot save keeps the `practice` key. The proof with the
  previous release of the app itself is Stage 2's.

## 9. Validation checklist (`jukeoke-cli verify`)

Magic, header CRC, file length, TOC CRC, every chunk CRC and bounds, manifest
schema, every referenced chunk present, every stem a valid FLAC stream with the
declared rate, channels, bit depth and length, lyrics times ordered and within the
song, each word's chars non-empty, slots valid or empty.

Version 1.1 adds, when the chunks are present (an absent guide is not a warning: guides
are optional):

- `guide`: valid JSON; `guide_version` 1 (a higher one: warning "made by a newer
  version", not checked further); `notes` ordered by `start`, each with
  `0 <= start < end <= duration_seconds + 0.05`, notes not overlapping by more than
  1 ms, `kind` and `octave` known values, `confidence` and `glide_in` in range,
  `midi` within 12..120 for pitched notes; `spans` ordered and inside the song;
  `tuning.cents` within -50..50 and `strength` within 0..1; `provenance.method` and
  `version` present. **Error** if `guide.curve` is named but missing or damaged.
- **Stale guide (warning):** `provenance.source.length` or `crc32` differs from the
  TOC entry of the `lead_vocal` stem (the stem was replaced after the guide was made).
  `--deep` also compares `source.sha256` with the stem chunk's bytes.
- `guide.curve`: magic, version, field table, exact length (section 10.3); every
  value finite; `MIDI` values 0 or within 12..120; `VOIC` within 0..1; the frames
  cover the song (`(frames - 1) x hop` within one hop of `duration_seconds`).
- Slot `practice` (when the active slot holds it): the types of section 11; at most 20
  `recent` results; unknown keys allowed.

## 10. The melody guide (`guide` and `guide.curve` chunks, 1.1)

The melody guide is what practice mode draws and grades against (Work Order 2 section
4.1): the notes the lead singer sang, measured from the isolated lead vocal, with the
pitch curve under them. It is optional: a file without it plays as before, and the app
can add it later (section 7.1). It is made by the shared Rust builder `jk-pitch::guide`
on any computer, the Mac included (DECISIONS D-236); the same file gives the same notes
everywhere because every reader uses the stored notes, never its own analysis.

### 10.1 The `guide` chunk (`application/json`)

```json
{
  "guide_version": 1,
  "hop_s": 0.01,
  "low_confidence": 0.5,
  "tuning": { "cents": -15.8, "strength": 0.91, "use_cents": -15.8 },
  "range": { "low": 52.31, "high": 76.12 },
  "counts": { "notes": 435, "pitched": 397, "unpitched": 38, "low_confidence": 64,
              "octave_flags": 12, "graded": 333 },
  "curve": "guide.curve",
  "notes": [
    { "start": 12.340, "end": 12.905, "midi": 64.273, "confidence": 0.931,
      "kind": "pitched", "octave": "ok", "glide_in": 0.040, "word": [3, 0, 12.340] },
    { "start": 12.905, "end": 13.400, "midi": 66.118, "confidence": 0.412,
      "kind": "pitched", "octave": "maybe_high", "glide_in": 0.0, "word": [3, 0, 12.340] },
    { "start": 80.120, "end": 80.410, "midi": 55.2, "confidence": 0.8,
      "kind": "unpitched", "octave": "unchecked", "glide_in": 0.0 }
  ],
  "spans": [ { "start": 80.120, "end": 85.600, "kind": "spoken" } ],
  "words": { "lyrics": "chunk", "offset_ms": 0, "word_count": 310 },
  "provenance": {
    "method": "jk-pitch pyin+notes",
    "version": "0.1.0",
    "parameters": { "fmin_hz": 60.0, "fmax_hz": 1100.0, "hop_s": 0.01, "window_s": 0.032,
                    "note_penalty": 16000.0, "min_note_s": 0.04, "low_confidence": 0.5,
                    "tuning_min_strength": 0.3, "...": "every tunable of the builder" },
    "source": { "role": "lead_vocal", "chunk": "stem.lead_vocal", "length": 21004311,
                "crc32": 2222222, "sha256": "...", "sample_rate": 48000, "channels": 2 },
    "analysis_rate_hz": 16000,
    "app": { "name": "Jukeoke", "version": "0.2.0", "platform": "windows", "arch": "x86_64" },
    "created_utc": "2026-11-20T10:00:00Z",
    "seconds": 1.17
  }
}
```

Times are song seconds (frame 0 of the stems is 0 s), written to 3 decimals; pitches are
MIDI note numbers with fractions (69.0 = A 440 Hz, 1.0 = one semitone, 0.01 = one cent),
written to 3 decimals (0.1 cent). Unknown keys are ignored everywhere.

**Notes** (`notes`, ordered by `start`, not overlapping):

| Key | Meaning |
|---|---|
| `start`, `end` | Where the note is sung (s). A scoop or slide into the note is part of it. |
| `midi` | The pitch **as measured**, never rounded to a piano key: the median of the note's steady part, the recording's tuning included (a song at A = 436 Hz has its A4 notes near 68.84). For an `unpitched` note: its median voiced pitch, used only to place the "speak" bar, or 0. |
| `confidence` | 0..1. Below `low_confidence` the note is drawn dim and never graded. A note flagged by the octave check has already had its confidence lowered (x 0.4 in builder 0.1.0). |
| `kind` | `pitched` or `unpitched` (spoken, rapped, shouted: drawn as "speak" bars, never graded on pitch). A reader treats an unknown value as `unpitched`. |
| `octave` | Result of the independent octave check (WO2 trap 8): `ok`, `maybe_high` (the true pitch may be an octave lower), `maybe_low` (an octave higher) or `unchecked` (unpitched or too short). A reader treats an unknown value as flagged. |
| `glide_in` | Seconds at the start of the note spent gliding into it (0 <= `glide_in` <= `end - start`). The curve shows the glide itself; a scorer gives no penalty there. |
| `word` | Optional: `[line, word, word_start]`, the word of the lyrics this note belongs to (section 10.4). Several notes may share one word (several notes on one syllable); a word may have no note. |

**Which notes are graded** (every reader applies exactly this rule): `kind` is
`pitched`, `confidence >= low_confidence`, `octave` is `ok` or `unchecked`, and the
note's midpoint is not inside a span the singer excluded (section 11). `counts.graded`
is that number before any exclusion.

**Spans** (`spans`, ordered): stretches drawn differently and never graded on pitch:
`spoken` (a spoken or rapped phrase, decided as a whole) and `breath` (a breath the
builder heard). Builder 0.1.0 writes `spoken` spans only; breaths are then simply gaps
between notes (nothing drawn, nothing graded). Unknown kinds are drawn as gaps.

**Tuning** (`tuning`): the recording's offset from A = 440 Hz in cents (-50..50) and its
strength (0..1, how consistently the notes agree on it). `use_cents` is the offset
readers use to line the piano keys up with the bars: `cents` when `strength >=
provenance.parameters.tuning_min_strength` (0.3 in builder 0.1.0), otherwise 0 (rap
songs measure 0.11-0.16: there is no tuning to find). It is decided once, in the file,
so every reader agrees. A key change shifts the bars and the keys together, so the
offset carries through every key.

**Range and counts**: informational summaries for lists and the lane's first layout
(`range` over pitched notes, unshifted). Readers recompute them when they need them
exactly.

**Key and octave are never stored in the guide.** The guide is always at the original
key and octave; practice mode adds the key heard (semitones) and the singer's octave
shift (multiples of 12) when it draws and grades, by one rule for both (docs/APP_API.md
section 12.1).

### 10.2 Provenance and replacing an old guide

`provenance` says what made the guide, so that a better method can replace old guides:

- `method` and `version`: the builder (`jk-pitch pyin+notes`, `0.1.0`). The builder's
  version changes whenever its output can change for the same input. A guide whose
  method differs from the app's builder, or whose version is older, is **outdated**:
  still used, and rebuilt when the singer asks ("Update practice data") or in a batch
  that includes outdated guides. A guide from a newer version than the app's own is
  kept, never downgraded.
- `parameters`: every tunable the builder used, so a result can be reproduced.
- `source`: the stem it was made from: `role`, `chunk` id, the chunk's `length` and
  `crc32` (as in the TOC) and `sha256` (of the chunk's bytes as stored), plus its
  `sample_rate` and `channels`. When `length` or `crc32` no longer match the
  `lead_vocal` stem's TOC entry, the guide is **stale** (the stem was replaced): it is
  not used for practice until it is rebuilt. Checking it costs nothing (the TOC is
  read anyway); `sha256` is for `verify --deep`.
- `analysis_rate_hz`, `app` (name, version, `windows` | `macos`, CPU architecture),
  `created_utc`, `seconds` (build time): for reports.

### 10.3 The `guide.curve` chunk (`application/octet-stream`)

The pitch curve under the notes, so slides and vibrato can be drawn and results checked:
one frame every `hop_us` microseconds (10 ms), frame `k` centred on song time
`t0_us + k x hop_us`. Little-endian binary, not compressed (random access by frame, no
codec; zlib would save about 190 KiB per 4-minute song, 0.13 % of the file):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `JKPC` (`4A 4B 50 43`) |
| 4 | 2 | curve version `1` |
| 6 | 2 | field count `F` (>= 1) |
| 8 | 4 | frame count `N` |
| 12 | 4 | `hop_us`: frame step in microseconds (`10000`) |
| 16 | 4 | `t0_us`: song time of frame 0 in microseconds, signed (`0`) |
| 20 | 12 | reserved, zero |
| 32 | 8 x F | field table: per field a 4-byte ASCII tag and a 4-byte format code (`1` = 32-bit IEEE float) |
| 32 + 8F | 4 x N x F | the fields' values, one array of `N` values per field, in field-table order (planar) |

Length is exactly `32 + 8F + 4NF` bytes. Fields defined in 1.1, written in this order
by 1.1 writers:

| Tag | Values |
|---|---|
| `MIDI` | pitch in MIDI units with fractions, tuning included; `0.0` = unvoiced (never NaN) |
| `VOIC` | voicing probability, 0..1 |
| `LVDB` | level of the analysis frame, dBFS (-120 for digital silence) |

Readers find fields by tag and skip unknown tags or format codes; a curve without
`MIDI` is treated as absent. `N = floor(duration_seconds / hop) + 1`.

### 10.4 Word references

A note's `word` is `[line, word, word_start]`: indices into `lines[line].words[word]` of
the **effective** lyrics (the slot's `lyrics_override` if present, else the `lyrics`
chunk) and that word's start time as used for the mapping (its stored `start` plus the
song's `lyrics_offset_ms` / 1000). `words` says what the mapping was made against:
`lyrics` (`chunk` or `override`), `offset_ms`, `word_count`.

The mapping is a display aid only. **It never moves, splits or deletes a note** (the
aligner can place a word seconds from where it is sung: WO2 trap 9). The rule, which
every implementation applies identically: for each note, the word with the largest
overlap between `[note.start, note.end]` and `[word.start + offset, word.end + offset]`
(the earlier word on a tie), kept only when that overlap is at least 30 % of the note's
length or at least 50 % of the word's length; otherwise the note has no word.

Stored references are a cache. A reader uses them only if `words.word_count` equals the
effective lyrics' word count, `words.offset_ms` equals the song's `lyrics_offset_ms`,
and every reference names an existing word whose start (plus the offset) equals
`word_start` within 1.5 ms; otherwise it recomputes all of them by the rule
(`jk-pitch` does this at practice start; a sync-editor correction therefore never makes
the guide wrong, only the cache old). Lyrics have no syllable boundaries, so a note
belongs to a word; a word sung over several notes is shared by those notes.

### 10.5 How the guide is made (builder 0.1.0)

From the `lead_vocal` stem as stored (decoded, stored gain applied, channels averaged,
decimated to about 16 kHz): a pYIN pitch curve (10 ms frames), notes cut at voicing
gaps, 6 dB level dips and pitch changes (vibrato flattened to its centre, glides kept in
the curve and folded into the next note), unpitched phrases detected as a whole, a
confidence per note, the spectral octave check, and the tuning (circular mean of the
confident notes' offsets). Details and measurements: `docs/benchmarks/practice-guide.md`
(Stage 0; D-236, D-237). The lyrics are read only for the word references. No model is
used; the build takes about 1.2 s for a 4-minute vocal on one core of the build PC
(estimated 1.7-2 s on the M1).

### 10.6 Size

Measured on the Stage 0 guides of the test songs (builder 0.1.0, every note given a
word reference to take the worst case; 2026-11-19, script and output kept under
`$JK_BUILD/practice-contracts/`):

| Song (file) | Length | Notes | `guide` JSON | `guide.curve` |
|---|---|---|---|---|
| Cortez - Feel (Stripped) | 240.6 s | 503 | 69.2 KiB | 281.9 KiB |
| coruscate - The Homeless Man | 245.7 s | 679 | 95.8 KiB | 288.0 KiB |
| Jason Miller - Crowdpleaser | 192.8 s | 593 | 84.1 KiB | 226.0 KiB |
| owner's song | 240.8 s | 435 | 60.4 KiB | 282.2 KiB |
| demo-song | 40.0 s | 42 | 8.3 KiB | 46.9 KiB |

**Budget for a 4-minute song: at most about 100 KiB of guide JSON plus 281 KiB of curve
(`32 + 24 + 12 x 24,001` bytes), about 0.4 MB**; the four real songs above take 0.25-0.32 % of their 110-142 MB files
measured. The curve grows by 1.2 KiB per second of song; the JSON by about 145 bytes per
note (word reference included). No model is added to the stick.

## 11. Practice data in the settings slot (1.1)

The singer's practice settings and results for a song travel with the song, in the
settings slot, under the user-state key `practice` (WO2 4.2, owner decision 9). They are
saved by the slot mechanism of section 6 (merge into the active state, write the other
slot), never in the web view's storage. Everything here is per song; per-computer
choices (the input device and channel, the latency offset, the lane's look) live in the
app settings (docs/APP_API.md section 12.7).

```json
"practice": {
  "practice_version": 1,
  "octave": { "mode": "auto", "shift": -1 },
  "strictness": null,
  "excluded": [ { "line": 12, "start": 80.12, "end": 85.60 } ],
  "lane": {},
  "best": { "...": "a result, below" },
  "recent": [ { "...": "results, newest first, at most 20" } ]
}
```

- `octave.mode`: `auto` (practice mode works out the singer's octave from the first
  sung phrases and moves the lane there; octaves are forgiven), `fixed` (the lane is
  moved by `shift` octaves; octaves are forgiven) or `strict` (the written octave is
  shown and required). `octave.shift`: -2..2, the shift in use (in `auto` the one found
  last time, the starting point next time). Default `{ "mode": "auto", "shift": 0 }`.
- `strictness`: `relaxed`, `standard`, `strict`, or `null` = the app's setting (the
  default). The bands belong to the scorer (default `standard`: full credit within 50
  cents, nothing from 100 cents; docs/APP_API.md 12.6).
- `excluded`: lines the singer marked "the guide is wrong here". Each is a time span
  (`start`, `end`, seconds), which is what counts: a note whose midpoint lies inside an
  excluded span is not graded. `line` is the index of the lyrics line it was made from,
  for display only (if the lyrics are corrected later, the span still covers the same
  music). Removing the exclusion removes the entry.
- `lane`: reserved for lane preferences that belong to the song; version 1 defines none
  (the lane's look depends on the computer and its screen, so those settings are per
  computer).
- `best`: the best result (highest `overall` among results with `sung_share >= 0.9`
  that are not `untrusted`), kept even after it leaves `recent`; `null` before there is
  one.
- `recent`: the 20 most recent results, newest first.

A **result**:

```json
{
  "date": "2026-11-20T19:05:00Z",
  "key": -3, "key_changes": 0, "seeks": 0, "octave": -1, "octave_mode": "auto",
  "strictness": "standard",
  "overall": 87.4, "grade": "B+",
  "graded": 212, "total": 240,
  "tendency_cents": -12,
  "sung_share": 0.98, "voiced_share": 0.91, "completed": true,
  "trust": ["music_in_input", "removal_active"],
  "guide": "jk-pitch pyin+notes 0.1.0", "scorer": "jk-pitch score 0.1.0",
  "app": "0.2.0",
  "lines": { "basis": "lyrics", "rows": [[0, 12.3, 92, 8, -5], [1, 15.9, null, 0, null]] }
}
```

| Key | Meaning |
|---|---|
| `date` | UTC time the take ended. |
| `key`, `key_changes`, `seeks` | The key heard (semitones, -12..12) for the largest share of the graded time; how many times the heard key changed during the take; how many seeks the take had (a note sung again keeps its latest attempt). |
| `octave`, `octave_mode` | The octave shift the singer sang in (-2..2) and the mode used. |
| `strictness` | The strictness used. |
| `overall`, `grade` | 0..100 (one decimal) and the letter grade; both `null` when the take could not be graded honestly (`trust` says why). |
| `graded`, `total` | "Graded on 212 of 240 notes": of the notes in the part of the song the take covered (`total`), how many were graded. |
| `tendency_cents` | The singer's typical offset from the guide on graded notes, in whole cents (positive = sharp); `null` if too little was sung. |
| `sung_share`, `voiced_share`, `completed` | How much of the song the take covered (share of the song's graded note time inside the played part); the share of that covered time in which the singer produced a pitch; whether the take ran to the end of the song. |
| `trust` | Why the result may not be fully trusted (empty = trusted). `music_in_input`: the app's own playback came back in the input; with it `removal_active` (removed well: graded normally), `removal_marginal` (removed only partly: graded, less certain) or `untrusted` (could not be removed, e.g. a wandering Bluetooth delay: not graded, `overall` null). `low_level`: the voice was too quiet for a reliable pitch for more than 10 % of the sung time. `clipping`: the input clipped. `uncertain_guide`: fewer than 60 % of the pitched notes in the covered part were gradable, or the guide is `outdated`. `latency_unmeasured`: no measured delay was available (only the declared device latencies and the manual offset were used). `input_lost`: the input stopped (unplugged, stalled) during the take. `dropouts`: input blocks were lost. `bluetooth_input`: a Bluetooth input was chosen by hand. `stopped_early`: the take ended before the song did. Readers show unknown flags as they are. |
| `guide`, `scorer`, `app` | The guide's method and version, the scorer's version and the app's version (results made with different guides or scorers are not compared as equals). |
| `lines` | The line-by-line breakdown. `basis`: `lyrics` (rows are lines of the effective lyrics at the time) or `phrases` (no lyrics: the guide's phrases, cut at gaps of 1 s or more). Each row is `[line, start, score, graded, tendency]`: the line index, its start (s, one decimal: lets a reader find the line again after the lyrics were corrected), its score as a whole number 0..100 (`null` when nothing in it was graded), its graded notes, its tendency in whole cents (`null` when none). |

**Number precision is part of the format** (the size bound below depends on it): `overall`
one decimal; `sung_share` and `voiced_share` two decimals; `tendency_cents` and every row's
score and tendency whole numbers; row `start` one decimal. Writers round before encoding;
unrounded doubles (17 significant digits each) would make the worst case below overflow a
slot.

**Size.** Worst case, computed 2026-11-19 (`measure_contracts.py` and its output under
`$JK_BUILD/practice-contracts/`): a song of 300 lyric lines (the test songs have 42-107),
every line excluded, 21 results (`best` + 20 `recent`) each with all 300 rows, every
trust flag and every other field at its longest value: the `practice` value alone is
**150.6 KiB as plain JSON (fits a slot even uncompressed), 43.3 KiB with zlib**.
Together with the largest other slot content, a `lyrics_override` three times the
largest lyrics chunk measured (3 x 79.9 KiB) and 150-character metadata overrides, the
whole user state is 392.5 KiB of JSON, which the slot writer stores with zlib (encoding
1, chosen automatically when plain JSON does not fit: section 6, `encode_slot` in Rust
and Python) in **101.9 KiB: 40 % of the 256 KiB slot**. A typical song (107 lines, 21
results, 3 exclusions) needs 55.3 KiB of plain JSON; one result with 107 rows is 2.6 KiB.
An independent recomputation with less compressible values (random row scores and
tendencies over their whole ranges, a 10-minute song, a 1,700-word `lyrics_override`
generated in the section 5 shape: `verifier_slot_worst.py` and its output in the same
folder) gives 124.0 KiB, 48 % of the slot; doubling everything (600 lines, a 20-minute song,
a 3,000-word override) still fits at 88 %. The same worst case with unrounded numbers needs
262 KiB and does not fit, hence the precision rule above.

**Never a whole-file rewrite for practice data.** If the encoded state would still not
fit a slot, the writer drops, in this order, the `rows` of the oldest `recent` results,
then the oldest `recent` results themselves, never `best`, until it fits. Only other
data (a `lyrics_override`) can make the app rewrite the file (section 6).

## 12. Version history

- **1.0** (2026-11-17): first version.
- **1.1** (2026-11-19, Work Order 2, practice mode): optional chunks `guide` and
  `guide.curve` (section 10) and the settings-slot key `practice` (section 11); header
  `minor` 1 and `format_version` `"1.1"` for new files; the rules for adding or
  replacing a guide in an existing file (section 7.1); validation additions (section 9).
  Minor change: 1.0 readers open and play 1.1 files and keep the new data when they
  write.
