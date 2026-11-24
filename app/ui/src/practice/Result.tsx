// SPDX-License-Identifier: GPL-3.0-or-later
// The result screen (WO2 4.7), in the jukebox look: the overall percentage in a chrome
// dial and the letter grade on a red-enamel lozenge; "graded on X of Y notes"; whether the
// singer tends sharp or flat and by about how much; the key and octave it was sung in and
// how much of the song was sung; the honesty notes when the result cannot be fully trusted;
// the line-by-line breakdown with the weak lines marked (and "the guide is wrong here" per
// graded line, saved with the song); the song's best and recent results; Sing again. Class
// hooks: .practice-result, .practice-result__dial, .practice-result__grade,
// .practice-result__row--weak, .practice-result__lines[data-more].
import { useEffect, useRef } from "preact/hooks";
import type { PracticeResult, SongDetail } from "../api/types";
import { lineText } from "../lib/lyrics";
import { closeResult, excludeLine, singAgain, stopPractice, usePractice } from "../state/practice";

/** Lines below this score are marked weak. */
export const WEAK_LINE = 60;

/**
 * "a little flat (about 12 cents)", "neither sharp nor flat on average", "sharp (about 35
 * cents)". The tendency is the typical signed offset only: a small one says the singer did
 * not lean either way, not that every note was in tune (that is what the score says), so the
 * words never claim "in tune" next to a low grade.
 */
export function tendencyText(cents: number | null): string {
  if (cents === null) return "not enough was sung to tell";
  const a = Math.round(Math.abs(cents));
  if (a < 6) return "neither sharp nor flat on average";
  const how = a < 20 ? "a little " : a < 45 ? "" : "clearly ";
  return `${how}${cents > 0 ? "sharp" : "flat"} (about ${a} cents)`;
}

export function octaveText(octave: number, mode: string): string {
  const where = octave === 0 ? "as written" : `${Math.abs(octave)} octave${Math.abs(octave) > 1 ? "s" : ""} ${octave < 0 ? "lower" : "higher"}`;
  return `${where}${mode === "auto" ? " (found automatically)" : mode === "strict" ? " (strict)" : ""}`;
}

function shortDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

function signedKey(k: number): string {
  return k === 0 ? "original key" : `${k > 0 ? "+" : "−"}${Math.abs(k)}`;
}

/** A line's lean shows from this many cents on (smaller is only in the tooltip). */
export const LINE_LEAN_CENTS = 15;

/**
 * A line's lean in words, "♭ flat" or "♯ sharp", with its cents in the tooltip. The sign
 * alone read as a stray glyph after the score (and says nothing to a screen reader).
 */
export function TendencyCell({ cents }: { cents: number | null }) {
  if (cents === null) return <span class="practice-result__ltend" role="cell" />;
  const a = Math.round(Math.abs(cents));
  const sharp = cents > 0;
  if (a < LINE_LEAN_CENTS) {
    return <span class="practice-result__ltend" role="cell" title={`${sharp ? "+" : "−"}${a} cents on average: no clear lean`} />;
  }
  return (
    <span class={`practice-result__ltend practice-result__ltend--${sharp ? "sharp" : "flat"}`} role="cell" title={`Sung ${sharp ? "sharp" : "flat"}: about ${a} cents ${sharp ? "high" : "low"} on average`}>
      <span aria-hidden="true">{sharp ? "♯" : "♭"}</span> {sharp ? "sharp" : "flat"}
    </span>
  );
}

/** The chrome ring around the score, filled to the score in gold. */
function Dial({ value, grade }: { value: number | null; grade: string | null }) {
  const r = 44;
  const c = 2 * Math.PI * r;
  const share = value === null ? 0 : Math.max(0, Math.min(1, value / 100));
  return (
    <div class="practice-result__dial">
      <svg viewBox="0 0 100 100" aria-hidden="true">
        <defs>
          <linearGradient id="pr-chrome" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#f7f9fc" />
            <stop offset="0.45" stop-color="#9aa3ae" />
            <stop offset="0.55" stop-color="#5a616b" />
            <stop offset="1" stop-color="#e6eaf0" />
          </linearGradient>
        </defs>
        <circle cx="50" cy="50" r="49" fill="url(#pr-chrome)" />
        <circle cx="50" cy="50" r="45.5" fill="#0d0b12" />
        <circle cx="50" cy="50" r={r} fill="none" stroke="rgba(255,255,255,0.08)" stroke-width="5" />
        <circle
          class="practice-result__arc"
          cx="50"
          cy="50"
          r={r}
          fill="none"
          stroke="var(--sung)"
          stroke-width="5"
          stroke-linecap="round"
          stroke-dasharray={`${(c * share).toFixed(2)} ${c.toFixed(2)}`}
          transform="rotate(-90 50 50)"
        />
      </svg>
      <span class="practice-result__pct">{value === null ? "—" : `${Math.round(value)}`}<small>{value === null ? "" : "%"}</small></span>
      {grade && <span class="practice-result__grade">{grade}</span>}
    </div>
  );
}

export function PracticeResultScreen({ result, song }: { result: PracticeResult; song: SongDetail }) {
  const history = usePractice((s) => s.history);
  const excluded = usePractice((s) => s.state?.excluded ?? []);
  const lyrics = song.lyrics;
  const r = result;
  const rows = r.lines.rows;
  const lineLabel = (i: number) => (r.lines.basis === "lyrics" && lyrics?.lines[i] ? lineText(lyrics.lines[i]) : `Phrase ${i + 1}`);
  const lineSpan = (i: number, start: number) => {
    if (r.lines.basis === "lyrics" && lyrics?.lines[i]) return { start: lyrics.lines[i].start, end: lyrics.lines[i].end };
    const next = rows.find((x) => x[1] > start);
    return { start, end: next ? next[1] : song.summary.duration_s };
  };
  const isExcluded = (span: { start: number; end: number }) => excluded.some((x) => Math.abs(x.start - span.start) < 0.05 && Math.abs(x.end - span.end) < 0.05);
  // Counted as the rows below mark them: a line the singer excluded is not called weak.
  const weak = rows.filter((row) => row[2] !== null && row[2] < WEAK_LINE && !isExcluded(lineSpan(row[0], row[1]))).length;
  const recent = (history?.recent ?? []).slice(0, 6);
  const linesRef = useRef<HTMLDivElement>(null);
  /** "2 weak lines below": brings the first weak line into view in the breakdown. */
  const showWeak = () => linesRef.current?.querySelector(".practice-result__row--weak")?.scrollIntoView({ block: "nearest" });
  // The breakdown scrolls when the lines do not all fit: its edge fades where more lines
  // are hidden (data-more, practice.css), so a row cut by the edge reads as "scroll for
  // more", not as a broken list. Set on the element itself: no re-render on scroll.
  useEffect(() => {
    const el = linesRef.current;
    if (!el) return;
    const update = () => {
      const above = el.scrollTop > 1;
      const below = el.scrollTop + el.clientHeight < el.scrollHeight - 1;
      const more = above && below ? "both" : above ? "above" : below ? "below" : "";
      if (more) el.dataset.more = more;
      else delete el.dataset.more;
    };
    update();
    el.addEventListener("scroll", update, { passive: true });
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    ro?.observe(el);
    return () => {
      el.removeEventListener("scroll", update);
      ro?.disconnect();
    };
  }, [rows.length]);
  return (
    <section class="practice-result" aria-labelledby="practice-result-title">
      <header class="practice-result__head">
        <h3 id="practice-result-title">Your result</h3>
        {r.best && <span class="practice-result__best">New best for this song!</span>}
        {!r.saved && <span class="practice-result__unsaved">{r.sung_share < 0.1 ? "Too short to keep in the history." : "Could not be saved in the song (is the stick read-only?)."}</span>}
      </header>
      <div class="practice-result__top">
        <Dial value={r.overall} grade={r.grade} />
        <dl class="practice-result__facts">
          <dt>Graded</dt>
          <dd>
            on {r.graded} of {r.total} notes
          </dd>
          <dt>Tuning</dt>
          <dd>You sang {tendencyText(r.tendency_cents)}</dd>
          <dt>Key</dt>
          <dd>
            {r.key_label ?? signedKey(r.key)}
            {r.key_changes > 0 ? ` (changed ${r.key_changes} time${r.key_changes > 1 ? "s" : ""})` : ""}
          </dd>
          <dt>Octave</dt>
          <dd>{octaveText(r.octave, r.octave_mode)}</dd>
          <dt>Sung</dt>
          <dd>
            {Math.round(r.sung_share * 100)} % of the song{r.completed ? "" : " (stopped early)"}
            {/* sung_share is how much of the song the take covered; when the input was lost
                part way, say so here too, not only in the honesty notes (WO2 4.7). */}
            {r.trust.includes("input_lost") ? ", but the input stopped part way" : ""}
            {weak > 0 && (
              <>
                {"; "}
                <button type="button" class="practice-result__jump" onClick={showWeak}>
                  {weak} weak line{weak > 1 ? "s" : ""} below
                </button>
              </>
            )}
          </dd>
        </dl>
        <div class="practice-result__history">
          <h4>This song</h4>
          {history?.best ? (
            <p class="practice-result__hbest">
              Best <strong>{history.best.overall === null ? "—" : `${Math.round(history.best.overall)} %`}</strong> {history.best.grade ?? ""} · {shortDate(history.best.date)} · {signedKey(history.best.key)}
            </p>
          ) : (
            <p class="practice-result__hbest">No best yet (a best needs 90 % of the song).</p>
          )}
          <ol class="practice-result__recent">
            {recent.map((h, i) => (
              <li key={`${h.date}-${i}`}>
                <span>{shortDate(h.date)}</span>
                <span>{signedKey(h.key)}</span>
                <span class="practice-result__hscore">{h.overall === null ? "not graded" : `${Math.round(h.overall)} % ${h.grade ?? ""}`}</span>
              </li>
            ))}
          </ol>
        </div>
      </div>
      {r.trust_messages.length > 0 && (
        <ul class="practice-result__trust" aria-label="About this result">
          {r.trust_messages.map((m, i) => (
            <li key={i} class={r.trust[i] === "untrusted" || r.trust[i] === "removal_marginal" ? "practice-result__trust--warn" : ""}>
              {m}
            </li>
          ))}
        </ul>
      )}
      <div class="practice-result__lines" role="table" aria-label="Line by line" ref={linesRef}>
        {/* Two balanced columns when the screen is wide enough (CSS container query), in song order. */}
        <div class="practice-result__cols" role="rowgroup">
        {rows.map(([line, start, score, graded, tend]) => {
          const span = lineSpan(line, start);
          const ex = isExcluded(span);
          const isWeak = score !== null && score < WEAK_LINE && !ex;
          // Only a line that was graded can be graded wrongly ("not graded" lines have no
          // button), and an excluded one keeps its "Count again".
          const canExclude = ex || score !== null;
          return (
            <div key={`${line}-${start}`} class={`practice-result__row${isWeak ? " practice-result__row--weak" : ""}${ex ? " practice-result__row--excluded" : ""}`} role="row">
              <span class="practice-result__ltext" role="cell">
                {lineLabel(line)}
              </span>
              <span class="practice-result__lbar" role="cell" aria-hidden="true">
                <span style={{ width: `${score ?? 0}%` }} />
              </span>
              <span class="practice-result__lscore" role="cell">
                {ex ? "excluded" : score === null ? (graded === 0 ? "not graded" : "—") : `${score}`}
                {isWeak && <span class="practice-result__weak">weak</span>}
              </span>
              <TendencyCell cents={tend} />
              <span class="practice-result__lact" role="cell">
                {canExclude && (
                  <button
                    type="button"
                    class="practice-result__exclude"
                    aria-pressed={ex}
                    aria-label={`${ex ? "Count again" : "Guide wrong?"} “${lineLabel(line)}”`}
                    title={ex ? "Count this line again" : "The practice data is wrong here: stop counting this line (saved with the song)"}
                    onClick={() => void excludeLine(r.lines.basis === "lyrics" ? line : null, span.start, span.end, !ex)}
                  >
                    {ex ? "Count again" : "Guide wrong?"}
                  </button>
                )}
              </span>
            </div>
          );
        })}
        </div>
      </div>
      <footer class="practice-result__actions">
        <button type="button" class="button button--primary practice-result__again" onClick={() => void singAgain()}>
          Sing again
        </button>
        <button type="button" class="button" onClick={() => closeResult()}>
          Back to the lane
        </button>
        <button type="button" class="button" onClick={() => void stopPractice()}>
          Leave practice
        </button>
      </footer>
    </section>
  );
}
