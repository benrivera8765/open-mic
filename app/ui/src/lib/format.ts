// SPDX-License-Identifier: GPL-3.0-or-later
// Small pure formatting helpers (no DOM, unit-tested in Node).

/** "3:07" (minutes:seconds), "1:02:03" past an hour. Negative or NaN shows "0:00". */
export function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** Countdown text for instrumental breaks: rounds UP, so "0:01" shows until singing starts. */
export function formatCountdown(seconds: number): string {
  return formatTime(Math.ceil(Math.max(0, seconds)));
}

/** "18.7 GB", "640 MB", "12 KB" (decimal units, like the OS file managers). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "?";
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u += 1;
  }
  return u === 0 ? `${v} ${units[0]}` : `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

/** "48 kHz", "44.1 kHz" */
export function formatRate(hz: number): string {
  const k = hz / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(1)} kHz`;
}

/** Human description of a sample format. */
export function formatSampleFormat(f: "f32" | "i16" | "i24" | "i32"): string {
  return { f32: "32-bit float", i16: "16-bit", i24: "24-bit", i32: "32-bit integer" }[f];
}

/** "-6.0 dB"; very quiet values show "-inf". */
export function formatDb(db: number): string {
  if (!Number.isFinite(db) || db <= -120) return "-inf dB";
  return `${db.toFixed(1)} dB`;
}

/**
 * The plain-language note for an export the core had to scale down (`export://done`
 * `applied_gain_db` < 0: a FLAC export whose mix peaked above full scale). Null when the
 * file holds the mix at its exact level. WORK_ORDER 5: no hidden processing.
 */
export function exportGainNotice(appliedGainDb: number | undefined, path: string): string | null {
  if (appliedGainDb === undefined || !Number.isFinite(appliedGainDb) || appliedGainDb >= 0) return null;
  const cut = -appliedGainDb;
  // 1 decimal normally; small cuts must not round to "0.0 dB".
  const amount = cut >= 0.095 ? `${cut.toFixed(1)} dB` : cut >= 0.005 ? `${cut.toFixed(2)} dB` : "less than 0.01 dB";
  const flac = /\.flac$/i.test(path);
  return (
    `The mix peaked above full scale, so the ${flac ? "FLAC " : ""}file was lowered by ${amount} to avoid clipping.` +
    (flac ? " Choose 32-bit float WAV to keep the exact level." : "")
  );
}

/** "+40 ms", "-15 ms", "0 ms" */
export function formatOffset(ms: number): string {
  const r = Math.round(ms);
  return `${r > 0 ? "+" : ""}${r} ms`;
}

/** Percent of a 0..1 value, "75 %". */
export const percent = (v: number): string => `${Math.round(v * 100)} %`;

/** Jukebox selection code for the n-th card: A1..A9, B1..., skipping I and O like real jukeboxes. */
export function selectionCode(index: number): string {
  const letters = "ABCDEFGHJKLMNPQRSTUVWXYZ";
  const letter = letters[Math.floor(index / 9) % letters.length];
  const round = Math.floor(index / (9 * letters.length));
  return `${letter}${(index % 9) + 1}${round > 0 ? `'`.repeat(round) : ""}`;
}
