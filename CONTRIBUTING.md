# Contributing to Jukeoke

Thank you for helping. Jukeoke is GPL-3.0-or-later; by contributing you agree that your
contribution is licensed the same way.

## Set up

- **Windows** (the full project, including the Studio): Rust stable for
  `x86_64-pc-windows-gnu`, WinLibs GCC, Node 24+, uv and Git Bash. No Visual Studio is
  needed (DECISIONS D-10). `source scripts/devenv.sh` puts the tools on the PATH for one
  shell and sets `$JK_BUILD` (`../jukeoke-build`, heavy local state kept out of the
  repository). The Studio's Python environment: [`studio/README.md`](studio/README.md).
- **macOS** (player, recorder): `bash scripts/build-mac.sh`, or `app/scripts/check-mac.sh`
  from Windows to type-check the macOS code.
- Build the UI before the app: `cd app/ui && npm ci && npm run build`.

## Tests to run before you send a change

```bash
cargo test --workspace                          # Rust: DSP accuracy, engine on a silent device, song format, app core
(cd app/ui && npm test && npm run build)        # UI: vitest incl. the ticker letter-under-marker check; ES2022 / Safari 16.4 checks
(cd studio && python -I -m pytest)              # Studio on the CPU with stand-in models (add -m gpu on an NVIDIA PC)
bash tests/conformance/run.sh                   # Python writer <-> Rust reader of .jksong
app/scripts/check-mac.sh                        # the macOS code still type-checks
scripts/gen-third-party.sh --check              # THIRD_PARTY_LICENSES.md matches the lock files
```

The app itself: `jukeoke --smoke-test` (window and UI round trip) and
`jukeoke --selfcheck --silent` (the self-check without sound). Any test that plays audio
must stay at or below -60 dBFS; device tests play digital silence.

## The contracts

The parts of the project meet at four documents. Change the document first, in the same
change as the code, and keep old files working (unknown fields are ignored, never an
error):

- [`docs/SONG_FORMAT.md`](docs/SONG_FORMAT.md): the `.jksong` file;
- [`docs/STUDIO_PROTOCOL.md`](docs/STUDIO_PROTOCOL.md): app <-> Studio messages;
- [`docs/APP_API.md`](docs/APP_API.md): UI <-> app core commands and events;
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): how it fits together.

Record a decision that someone might later question in [`DECISIONS.md`](DECISIONS.md), with
one line of reasoning.

## Rules the code keeps

- **Audio quality first**: 32-bit float processing, no lossy encoding anywhere, resample at
  most once and say where, no hidden normalisation, limiting or EQ. Every audio claim needs a
  measured number (`docs/benchmarks/`).
- **The real-time audio path** neither allocates nor locks; the UI never touches audio.
- **Portable**: nothing depends on an absolute path or a drive letter; everything is written
  under the stick's root, atomically (temporary file, then rename).
- **Mac**: macOS 13.0 is the minimum; any newer API goes behind an availability check; the UI
  works in Safari 16.4.
- **Names**: the app name lives only in `branding.json`.

## Code style

- Rust: rustfmt style and no new `cargo clippy` warnings. The tree is not fully
  rustfmt-formatted yet (`cargo fmt --all --check` reports differences), so format the files
  you touch (`rustfmt <file>`) rather than the whole workspace in an unrelated change.
- TypeScript: strict mode, `tsc` clean (`npm run build` checks it).
- Python: standard library first, type hints, run with `python -I`.
- Shell: bash, `set -euo pipefail`, clean under `shellcheck`.
- New files start with `SPDX-License-Identifier: GPL-3.0-or-later` (or the licence of code
  they derive from, with its notice).
- Comments say why, in plain words.

## What must never be committed

- **No copyrighted music, lyrics or photographs**, not even short test clips. Test audio is
  generated in code or Creative Commons with attribution; test lyrics are invented or public
  domain. Keep downloaded test songs and datasets in `$JK_BUILD`, outside the repository.
- **No brand names, logos or artwork** of real jukebox makers. The look is drawn in code.
- No model weights (they are downloaded and checked by SHA-256; see `MODELS.md`), no audio
  files, no secrets, no personal paths, no files over 5 MB.
- A new dependency needs a licence compatible with GPL-3.0-or-later; run
  `scripts/gen-third-party.sh` and commit the updated `THIRD_PARTY_LICENSES.md`. A new model
  goes into `studio/jukeoke_studio/modelzoo/registry.py` and `MODELS.md` with its licence.

## Sign-off (optional)

You may add a `Signed-off-by: Your Name <email>` line (`git commit -s`) to certify the
[Developer Certificate of Origin](https://developercertificate.org/): that you wrote the
change or have the right to submit it under the project's licence. It is welcome, not
required.
