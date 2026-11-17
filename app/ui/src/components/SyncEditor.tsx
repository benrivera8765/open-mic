// SPDX-License-Identifier: GPL-3.0-or-later
// Sync editor (WORKING ORDER 4.1 "Sync editor"): global timing offset, per-device
// Bluetooth offset, nudge a line, drag a line on a timeline, tap along to re-time a
// line, correct the text, "Re-align with Studio" where the Studio exists, save into
// the song file (lyrics_save) or reset to the song's own lyrics (lyrics_reset).
// The manual tools work everywhere, including the Mac.
//
// Times written by tap-along are lyric times: the playback position at the tap minus
// the song and device offsets, so a re-tapped line lands where it was heard.
//
// Class hooks: .sync, .sync__offsets, .sync__timeline, .sync__track, .sync__block,
// .sync__block--selected, .sync__playhead, .sync__lines, .sync__line,
// .sync__line--selected, .sync__tap, .sync__tap-word, .sync__tap-word--done,
// .sync__actions.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { Lyrics } from "../api/types";
import { errorMessage } from "../api/client";
import { formatOffset, formatTime } from "../lib/format";
import { cloneLyrics, editLineText, lineMoveBounds, lineText, moveLineTo, realignRequest, retimeLineFromTaps, shiftLine, validateLyrics, wordText } from "../lib/lyrics";
import { changeSettings, changeSongSettings, clock, deviceOffsetKey, deviceOffsetMs, getApiSync, go, play, resetLyrics, saveLyrics, seek, store, toast, useApp } from "../state/app";
import { Ticker } from "../ticker/Ticker";
import { Note, Section, Slider, Toggle } from "./common";
import { usePosition } from "./Transport";

/** Working copy per song, kept while the user moves between screens. */
interface Draft {
  songId: string;
  /** The song's lyrics object this copy was made from (to notice a reload or reset). */
  base: Lyrics;
  lyrics: Lyrics;
  undo: Lyrics[];
  redo: Lyrics[];
  anchored: Set<number>;
  dirty: boolean;
}
let draft: Draft | null = null;

function useDraft(): [Draft | null, (f: (d: Draft) => Partial<Draft>) => void] {
  const song = useApp((s) => s.song);
  const [, force] = useState(0);
  // A new copy when another song is loaded, or when the song's lyrics were replaced
  // (reset to original, song reloaded) while there are no unsaved edits. After a save
  // the song's lyrics ARE the draft's lyrics, so the working copy (and undo) is kept.
  const replaced = !!draft && !!song?.lyrics && song.lyrics !== draft.base && song.lyrics !== draft.lyrics && !draft.dirty;
  if (song?.lyrics && (!draft || draft.songId !== song.summary.song_id || replaced)) {
    draft = { songId: song.summary.song_id, base: song.lyrics, lyrics: cloneLyrics(song.lyrics), undo: [], redo: [], anchored: new Set(), dirty: false };
  }
  if (!song?.lyrics) draft = null;
  const update = (f: (d: Draft) => Partial<Draft>) => {
    if (!draft) return;
    draft = { ...draft, ...f(draft) };
    force((n) => n + 1);
  };
  return [draft, update];
}

/** Records an edit: pushes the previous lyrics on the undo stack. */
function edit(d: Draft, next: Lyrics, anchorLine?: number): Partial<Draft> {
  const anchored = new Set(d.anchored);
  if (anchorLine !== undefined) anchored.add(anchorLine);
  return { lyrics: next, undo: [...d.undo.slice(-49), d.lyrics], redo: [], anchored, dirty: true };
}

function Offsets() {
  const st = useApp((s) => s.songSettings);
  const settings = useApp((s) => s.settings);
  const deviceName = useApp((s) => s.player?.device?.name ?? "this output");
  if (!st || !settings) return null;
  const songOff = st.lyrics_offset_ms;
  const devOff = deviceOffsetMs(settings);
  const setSong = (v: number) => changeSongSettings({ lyrics_offset_ms: Math.round(Math.max(-5000, Math.min(5000, v))) }, 80);
  const setDev = (v: number) => changeSettings({ audio: { device_offsets_ms: { [deviceOffsetKey(settings)]: Math.round(Math.max(-2000, Math.min(2000, v))) } } }, 300);
  return (
    <div class="sync__offsets">
      <div class="sync__offset">
        <Slider label="Song timing offset (positive = lyrics later)" value={songOff} min={-2000} max={2000} step={10} format={formatOffset} onInput={setSong} />
        <div class="row">
          {[-50, -10, 10, 50].map((d) => (
            <button key={d} type="button" class="button button--small" onClick={() => setSong(songOff + d)}>
              {d > 0 ? `+${d}` : d} ms
            </button>
          ))}
          <button type="button" class="button button--small" disabled={songOff === 0} onClick={() => setSong(0)}>
            0
          </button>
        </div>
        <p class="hint">Saved with this song. Moves all lyrics at once.</p>
      </div>
      <div class="sync__offset">
        <Slider label={`Output delay for ${deviceName}`} value={devOff} min={0} max={1000} step={10} format={formatOffset} onInput={setDev} />
        <p class="hint">Bluetooth headphones and speakers play late. Set this once per device; it applies to every song.</p>
      </div>
    </div>
  );
}

/** Timeline of line blocks; drag a block to move its line in time; click to seek. */
function Timeline({ d, selected, onSelect, onMove, duration }: { d: Draft; selected: number; onSelect: (i: number) => void; onMove: (i: number, start: number) => void; duration: number }) {
  const [pps, setPps] = useState(40); // pixels per second
  const pos = usePosition(15);
  const [dragging, setDragging] = useState<{ i: number; start: number } | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const lines = d.lyrics.lines;

  // Keep the playhead in view while playing.
  useEffect(() => {
    const el = scroller.current;
    if (!el || dragging) return;
    const x = pos * pps;
    if (x < el.scrollLeft + 40 || x > el.scrollLeft + el.clientWidth - 80) el.scrollLeft = Math.max(0, x - el.clientWidth / 3);
  }, [Math.floor(pos * pps / 200), pps]);

  const startDrag = (e: PointerEvent, i: number) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    onSelect(i);
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    const s0 = lines[i].start;
    const { minStart, maxStart } = lineMoveBounds(d.lyrics, i, duration);
    let cur = s0;
    const move = (ev: PointerEvent) => {
      cur = Math.max(minStart, Math.min(maxStart, s0 + (ev.clientX - x0) / pps));
      setDragging({ i, start: cur });
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      setDragging(null);
      if (Math.abs(cur - s0) > 0.0005) onMove(i, cur);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  return (
    <div class="sync__timeline">
      <div class="row">
        <Slider label="Zoom" value={pps} min={10} max={240} step={5} format={(v) => `${v} px/s`} onInput={setPps} />
      </div>
      <div class="sync__scroller" ref={scroller}>
        <div
          class="sync__track"
          style={{ width: `${Math.max(duration, 1) * pps}px` }}
          onClick={(e) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            void seek((e.clientX - r.left) / pps);
          }}
        >
          {Array.from({ length: Math.floor(duration / 10) + 1 }, (_, k) => (
            <span key={k} class="sync__second" style={{ left: `${k * 10 * pps}px` }}>
              {formatTime(k * 10)}
            </span>
          ))}
          {lines.map((l, i) => {
            const start = dragging?.i === i ? dragging.start : l.start;
            return (
              <button
                key={i}
                type="button"
                class={`sync__block${i === selected ? " sync__block--selected" : ""}${d.anchored.has(i) ? " sync__block--anchored" : ""}`}
                style={{ left: `${start * pps}px`, width: `${Math.max(4, (l.end - l.start) * pps)}px` }}
                title={`${formatTime(start)}: ${lineText(l)} (drag to move)`}
                aria-label={`Line ${i + 1} at ${start.toFixed(2)} s: ${lineText(l)}. Drag to move.`}
                onPointerDown={(e) => startDrag(e as unknown as PointerEvent, i)}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(i);
                }}
              >
                <span>{lineText(l)}</span>
              </button>
            );
          })}
          <span class="sync__playhead" style={{ left: `${pos * pps}px` }} aria-hidden="true" />
        </div>
      </div>
    </div>
  );
}

/** Tap-along: Space/Enter or the big button at the start of each word of one line. */
function TapAlong({ d, line, onDone, onCancel }: { d: Draft; line: number; onDone: (taps: number[]) => void; onCancel: () => void }) {
  const [taps, setTaps] = useState<number[]>([]);
  const words = d.lyrics.lines[line].words;
  const settings = useApp((s) => s.settings);
  const songOff = useApp((s) => s.songSettings?.lyrics_offset_ms ?? 0);
  const offset = (songOff + deviceOffsetMs(settings)) / 1000;
  const tapsRef = useRef(taps);
  tapsRef.current = taps;

  const tap = (atMs: number) => {
    const t = clock.position(atMs) - offset;
    const next = [...tapsRef.current, t];
    setTaps(next);
    if (next.length >= words.length) onDone(next);
  };

  useEffect(() => {
    store.set({ tapMode: true });
    const lineStart = d.lyrics.lines[line].start;
    void seek(Math.max(0, lineStart - 2.5)).then(() => play());
    const onKey = (e: KeyboardEvent) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        if (!e.repeat) tap(e.timeStamp || performance.now());
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      } else if (e.key === "Backspace") {
        e.preventDefault();
        setTaps((t) => t.slice(0, -1));
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      store.set({ tapMode: false });
    };
  }, [line]);

  return (
    <div class="sync__tap" role="dialog" aria-label="Tap along">
      <p>
        Tap <kbd>Space</kbd> (or the button) as each word starts. <kbd>Backspace</kbd> takes back a tap, <kbd>Esc</kbd> cancels.
      </p>
      <p class="sync__tap-words">
        {words.map((w, k) => (
          <span key={k} class={`sync__tap-word${k < taps.length ? " sync__tap-word--done" : ""}${k === taps.length ? " sync__tap-word--next" : ""}`}>
            {wordText(w)}{" "}
          </span>
        ))}
      </p>
      <div class="row">
        <button type="button" class="button button--primary sync__tap-button" onPointerDown={(e) => {
            e.preventDefault();
            tap(e.timeStamp || performance.now());
          }}>
          Tap
        </button>
        <button type="button" class="button" disabled={taps.length === 0} onClick={() => onDone(taps)}>
          Use {taps.length} tap{taps.length === 1 ? "" : "s"}
        </button>
        <button type="button" class="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

function LineRow({ d, i, selected, onSelect, apply }: { d: Draft; i: number; selected: boolean; onSelect: () => void; apply: (next: Lyrics, anchor?: number) => void }) {
  const l = d.lyrics.lines[i];
  const [text, setText] = useState(lineText(l));
  const duration = useApp((s) => s.song?.summary.duration_s ?? 0);
  useEffect(() => setText(lineText(l)), [l]);
  const commit = () => {
    if (text !== lineText(l)) apply(editLineText(d.lyrics, i, text));
  };
  const nudge = (dt: number) => apply(shiftLine(d.lyrics, i, dt, duration), i);
  return (
    <li class={`sync__line${selected ? " sync__line--selected" : ""}`} onFocusCapture={onSelect}>
      <span class="sync__line-time">{l.start.toFixed(2)}</span>
      <label class="visually-hidden" for={`line-${i}`}>
        Text of line {i + 1}
      </label>
      <input
        id={`line-${i}`}
        class="sync__line-text"
        value={text}
        onInput={(e) => setText((e.currentTarget as HTMLInputElement).value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commit();
            (e.currentTarget as HTMLInputElement).blur();
          } else if (e.key === "Escape") setText(lineText(l));
        }}
      />
      <span class="sync__line-buttons">
        <button type="button" class="button button--small" aria-label={`Line ${i + 1} earlier by 50 ms`} onClick={() => nudge(-0.05)}>
          −50 ms
        </button>
        <button type="button" class="button button--small" aria-label={`Line ${i + 1} later by 50 ms`} onClick={() => nudge(0.05)}>
          +50 ms
        </button>
        <button type="button" class="button button--small" aria-label={`Play from line ${i + 1}`} onClick={() => void seek(Math.max(0, l.start - 2)).then(() => play())}>
          ▶ Play
        </button>
      </span>
    </li>
  );
}

export function SyncEditor() {
  const song = useApp((s) => s.song);
  const st = useApp((s) => s.songSettings);
  const ui = useApp((s) => s.settings?.ui);
  const devOff = useApp((s) => deviceOffsetMs(s.settings));
  const studioOk = useApp((s) => s.info?.capabilities.studio.available ?? false);
  const [d, update] = useDraft();
  const [selected, setSelected] = useState(0);
  const [tapLine, setTapLine] = useState<number | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const problems = useMemo(() => (d ? validateLyrics(d.lyrics, song?.summary.duration_s) : []), [d?.lyrics]);

  if (!song || !st || !ui) {
    return (
      <Section title="Fix lyrics timing" class="sync">
        <p>Load a song first.</p>
      </Section>
    );
  }
  if (!d) {
    return (
      <Section title="Fix lyrics timing" class="sync">
        <p>This song has no lyrics to correct. Lyrics are added in the Studio.</p>
      </Section>
    );
  }
  const duration = song.summary.duration_s;
  const apply = (next: Lyrics, anchor?: number) => update((x) => edit(x, next, anchor));

  const realign = async () => {
    const { text, anchors } = realignRequest(d.lyrics, d.anchored);
    setBusy("Re-aligning with the Studio…");
    try {
      const out = await getApiSync().studioRealign(song.summary.song_id, text, anchors);
      apply({ ...out, checked: d.lyrics.checked });
      toast("Re-aligned. Check it, then save.");
    } catch (err) {
      toast(`Re-align failed: ${errorMessage(err)}`, "error");
    } finally {
      setBusy(null);
    }
  };

  const save = async () => {
    if (problems.length) return;
    setBusy("Saving…");
    const ok = await saveLyrics(d.lyrics);
    setBusy(null);
    if (ok) update(() => ({ dirty: false }));
  };

  return (
    <div class="sync">
      <Section
        title="Fix lyrics timing"
        actions={
          <button type="button" class="button" onClick={() => go("stage")}>
            Back to the stage
          </button>
        }
      >
        <div class="sync__preview">
          <Ticker lyrics={d.lyrics} offsetS={(st.lyrics_offset_ms + devOff) / 1000} textSize={Math.min(ui.lyrics_text_size, 44)} marker={ui.marker_position} colors={ui.colors} />
        </div>
        <Offsets />
      </Section>

      <Section title="Lines">
        <Timeline d={d} selected={selected} onSelect={setSelected} onMove={(i, start) => apply(moveLineTo(d.lyrics, i, start, duration), i)} duration={duration} />
        {tapLine !== null ? (
          <TapAlong
            d={d}
            line={tapLine}
            onCancel={() => setTapLine(null)}
            onDone={(taps) => {
              apply(retimeLineFromTaps(d.lyrics, tapLine, taps, duration), tapLine);
              setTapLine(null);
            }}
          />
        ) : (
          <div class="row sync__selected-tools">
            <span>
              Line {selected + 1}: “{d.lyrics.lines[selected] ? lineText(d.lyrics.lines[selected]) : ""}”
            </span>
            <button type="button" class="button" disabled={!d.lyrics.lines[selected]} onClick={() => setTapLine(selected)}>
              Tap along to re-time this line
            </button>
          </div>
        )}
        <ol class="sync__lines">
          {d.lyrics.lines.map((_, i) => (
            <LineRow key={i} d={d} i={i} selected={i === selected} onSelect={() => setSelected(i)} apply={apply} />
          ))}
        </ol>
      </Section>

      <Section title="Save">
        <Toggle label="I have checked these lyrics" checked={d.lyrics.checked} onChange={(v) => apply({ ...cloneLyrics(d.lyrics), checked: v })} />
        {problems.length > 0 && (
          <Note kind="error">
            Cannot save yet: {problems.slice(0, 3).join("; ")}
            {problems.length > 3 ? ` (and ${problems.length - 3} more)` : ""}
          </Note>
        )}
        <div class="row sync__actions">
          <button type="button" class="button" disabled={d.undo.length === 0} onClick={() => update((x) => ({ lyrics: x.undo[x.undo.length - 1], undo: x.undo.slice(0, -1), redo: [...x.redo, x.lyrics], dirty: true }))}>
            Undo
          </button>
          <button type="button" class="button" disabled={d.redo.length === 0} onClick={() => update((x) => ({ lyrics: x.redo[x.redo.length - 1], redo: x.redo.slice(0, -1), undo: [...x.undo, x.lyrics], dirty: true }))}>
            Redo
          </button>
          {studioOk ? (
            <button type="button" class="button" disabled={busy !== null} onClick={() => void realign()} title="Times every letter again from the singer's voice, keeping the lines you moved by hand where you put them">
              Re-align with Studio
            </button>
          ) : (
            <span class="hint">Re-aligning needs the Studio (a Windows computer with an NVIDIA card). The manual tools work everywhere.</span>
          )}
          <button type="button" class="button button--primary" disabled={busy !== null || problems.length > 0 || !d.dirty} onClick={() => void save()}>
            Save into the song
          </button>
          {confirmReset ? (
            <span class="sync__confirm" role="alert">
              Throw away all corrections and use the song's original lyrics?{" "}
              <button
                type="button"
                class="button button--danger"
                onClick={() => {
                  // Drop the working copy; it is rebuilt from the song's lyrics on the next render.
                  draft = null;
                  if (song.lyrics_is_override) void resetLyrics();
                  setConfirmReset(false);
                }}
              >
                Yes, reset
              </button>{" "}
              <button type="button" class="button" onClick={() => setConfirmReset(false)}>
                Keep
              </button>
            </span>
          ) : (
            <button type="button" class="button" disabled={!song.lyrics_is_override && !d.dirty} onClick={() => setConfirmReset(true)}>
              Reset to original
            </button>
          )}
        </div>
        {busy && <p role="status">{busy}</p>}
        {d.dirty && !busy && <p class="hint">Unsaved changes. They show in the preview above but are not in the song file yet.</p>}
      </Section>
    </div>
  );
}
