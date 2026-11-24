// SPDX-License-Identifier: GPL-3.0-or-later
// The console of the jukebox look (settings.ui.skin on): the controls that sit in
// the cabinet under the screen (docs/DESIGN.md "Window anatomy"), built from the
// skin's parts (src/skin) and wired to the same actions as the plain layout
// (state/app.ts), so both looks behave the same:
//
//   [restart][back 5 s][play/pause][forward 5 s]    (( KEY DIAL ))    GUIDE BACKING MASTER
//   [========== seek ==========] 0:42 / 2:14       A major → B major      faders   [clip]
//
// The key dial replaces the plain key control's visuals only: it calls the same
// setKey/shiftKey actions, the app-wide shortcuts ([ ] - + in App.tsx) keep working,
// and it adds dragging, the wheel, PageUp/PageDown, Home/End and 0 = original key.
//
// Class hooks: .skin-console, .skin-console__transport, .skin-console__key,
// .skin-console__mix, .skin-transport__plates, .skin-transport__restart,
// .skin-transport__back, .skin-transport__play, .skin-transport__forward,
// .skin-transport__seek, .skin-transport__time, .skin-clip, .skin-clip--on. (The plain
// transport's class names are not reused: style.css would restyle the plates.)
import { useEffect, useState } from "preact/hooks";
import type { Api } from "../api/client";
import { formatTime, percent } from "../lib/format";
import { keyLabel } from "../lib/keys";
import { Fader, KeyDial, PlateButton, TransportIcon } from "../skin";
import { KeyPrepareNote } from "./KeyControl";
import { changeSongSettings, restart, seek, seekBy, setKey, setMaster, togglePlay, useApp } from "../state/app";
import { usePosition } from "./Transport";

/**
 * True while the native window is minimised (Tauri reports it; a web view does not
 * always turn that into a hidden page). The frame stops its light drift and bubbles.
 */
export function useWindowMinimised(api: Api): boolean {
  const [minimised, setMinimised] = useState(false);
  useEffect(() => api.onWindowMinimised?.(setMinimised), [api]);
  return minimised;
}

function SkinTransport() {
  const song = useApp((s) => s.song);
  const state = useApp((s) => s.player?.state ?? "empty");
  const pos = usePosition(4);
  const [drag, setDrag] = useState<number | null>(null);
  const dur = song?.summary.duration_s ?? 0;
  const disabled = !song;
  const playing = state === "playing";
  const shown = drag ?? pos;
  return (
    <div class="skin-console__transport" role="group" aria-label="Playback">
      <div class="skin-transport__plates">
        <PlateButton class="skin-transport__restart" label={<TransportIcon name="restart" />} ariaLabel="Restart from the beginning" title="Restart" disabled={disabled} onClick={() => void restart()} />
        <PlateButton class="skin-transport__back" label={<TransportIcon name="back5" />} ariaLabel="Back 5 seconds" title="Back 5 s (←)" disabled={disabled} onClick={() => void seekBy(-5)} />
        <PlateButton
          class="skin-transport__play"
          size="lg"
          label={<TransportIcon name={playing ? "pause" : "play"} />}
          ariaLabel={playing ? "Pause" : "Play"}
          title={playing ? "Pause (Space)" : "Play (Space)"}
          active={playing}
          disabled={disabled || state === "loading"}
          onClick={() => void togglePlay()}
        />
        <PlateButton class="skin-transport__forward" label={<TransportIcon name="forward5" />} ariaLabel="Forward 5 seconds" title="Forward 5 s (→)" disabled={disabled} onClick={() => void seekBy(5)} />
      </div>
      <div class="skin-transport__seek">
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
        <span class="skin-transport__time" aria-hidden="true">
          {formatTime(shown)} / {formatTime(dur)}
        </span>
      </div>
    </div>
  );
}

function SkinKey() {
  const song = useApp((s) => s.song);
  const semis = useApp((s) => s.songSettings?.semitones ?? 0);
  // Same label as the plain key control ("A major → B major (+2)"), computed from the
  // song's detected key, so it updates the moment the key changes.
  const label = song ? keyLabel(song.summary.key, semis) : "No song loaded";
  // One dial per song: the dial commits a turn 160 ms after the hand stops, and a
  // turn still waiting when another song opens must not land on (and be saved into)
  // the new song. A new `key` unmounts the old dial, which cancels its timer.
  return (
    <div class="skin-console__key" data-semitones={semis}>
      <KeyDial key={song?.summary.song_id ?? ""} semitones={semis} label={label} onChange={setKey} disabled={!song} />
      <KeyPrepareNote />
    </div>
  );
}

function SkinMix() {
  const song = useApp((s) => s.song);
  const st = useApp((s) => s.songSettings);
  const master = useApp((s) => s.settings?.audio.master_volume ?? 1);
  const clipped = useApp((s) => s.player?.clipped ?? false);
  const roles = new Set(song?.stems.map((s) => s.role) ?? []);
  const off = !song || !st;
  const backingOff = off || !roles.has("backing_vocals") || !st?.backing_on;
  return (
    <div class="skin-console__mix" role="group" aria-label="Mix">
      <Fader
        label="Guide"
        title="Guide vocal: the original singer, 0 to 100 %"
        value={st?.guide_vocal ?? 0}
        valueText={percent(st?.guide_vocal ?? 0)}
        disabled={off || !roles.has("lead_vocal")}
        onChange={(v) => changeSongSettings({ guide_vocal: v }, 40)}
      />
      <Fader
        label="Backing"
        title={st && !st.backing_on ? "Backing vocals are off (switch them on in the mixer)" : "Backing vocals level"}
        value={st?.backing_level ?? 1}
        valueText={st && !st.backing_on ? "Off" : percent(st?.backing_level ?? 1)}
        disabled={backingOff}
        onChange={(v) => changeSongSettings({ backing_level: v }, 40)}
      />
      <Fader label="Master" title="Master volume (this computer)" value={master} valueText={percent(master)} onChange={(v) => void setMaster(v)} />
      <span class={`skin-clip${clipped ? " skin-clip--on" : ""}`} role="status" title={clipped ? "The output clipped: lower a level or the master volume." : "No clipping"}>
        <span class="skin-clip__lamp" aria-hidden="true" />
        <span class="skin-clip__text">{clipped ? "Clip!" : "Clip"}</span>
      </span>
    </div>
  );
}

/** Everything in the console slot of the JukeboxFrame. */
export function SkinConsole() {
  return (
    <div class="skin-console" aria-label="Player controls" role="group">
      <SkinTransport />
      <SkinKey />
      <SkinMix />
    </div>
  );
}
