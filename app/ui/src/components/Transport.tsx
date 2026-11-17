// SPDX-License-Identifier: GPL-3.0-or-later
// Transport: play/pause, restart, seek bar and time. Class hooks: .transport,
// .transport__play, .transport__restart, .transport__back, .transport__forward,
// .transport__seek, .transport__time.
import { useEffect, useState } from "preact/hooks";
import { formatTime } from "../lib/format";
import { clock, onClockKick, restart, seek, seekBy, togglePlay, useApp } from "../state/app";

/** Playback position for display, refreshed `hz` times per second while playing. */
export function usePosition(hz = 4): number {
  const [pos, setPos] = useState(() => clock.position(performance.now()));
  const playing = useApp((s) => s.player?.state === "playing");
  useEffect(() => {
    const update = () => setPos(clock.position(performance.now()));
    update();
    const off = onClockKick(() => {
      if (!clock.playing) update();
    });
    if (!playing) return off;
    const h = setInterval(update, 1000 / hz);
    return () => {
      off();
      clearInterval(h);
    };
  }, [playing, hz]);
  return pos;
}

export function Transport() {
  const song = useApp((s) => s.song);
  const state = useApp((s) => s.player?.state ?? "empty");
  const pos = usePosition(4);
  const [drag, setDrag] = useState<number | null>(null);
  const dur = song?.summary.duration_s ?? 0;
  const disabled = !song;
  const playing = state === "playing";
  const shown = drag ?? pos;

  return (
    <div class="transport" role="group" aria-label="Playback">
      <button type="button" class="transport__restart icon-button" aria-label="Restart from the beginning" title="Restart" disabled={disabled} onClick={() => void restart()}>
        <span aria-hidden="true">⏮</span>
      </button>
      <button type="button" class="transport__back icon-button" aria-label="Back 5 seconds" title="Back 5 s (←)" disabled={disabled} onClick={() => void seekBy(-5)}>
        <span aria-hidden="true">↺5</span>
      </button>
      <button
        type="button"
        class={`transport__play${playing ? " transport__play--playing" : ""}`}
        aria-label={playing ? "Pause" : "Play"}
        title={playing ? "Pause (Space)" : "Play (Space)"}
        disabled={disabled || state === "loading"}
        onClick={() => void togglePlay()}
      >
        <span aria-hidden="true">{playing ? "❚❚" : "▶"}</span>
      </button>
      <button type="button" class="transport__forward icon-button" aria-label="Forward 5 seconds" title="Forward 5 s (→)" disabled={disabled} onClick={() => void seekBy(5)}>
        <span aria-hidden="true">5↻</span>
      </button>
      <div class="transport__seek">
        <label for="seek" class="visually-hidden">
          Position in the song
        </label>
        <input
          id="seek"
          type="range"
          min={0}
          max={Math.max(dur, 0.1)}
          step={0.1}
          value={Math.min(shown, dur)}
          disabled={disabled}
          aria-valuetext={`${formatTime(shown)} of ${formatTime(dur)}`}
          onInput={(e) => setDrag(Number((e.currentTarget as HTMLInputElement).value))}
          onChange={(e) => {
            const v = Number((e.currentTarget as HTMLInputElement).value);
            setDrag(null);
            void seek(v);
          }}
          onKeyDown={(e) => e.stopPropagation()}
        />
      </div>
      <span class="transport__time" aria-hidden="true">
        {formatTime(shown)} / {formatTime(dur)}
      </span>
    </div>
  );
}
