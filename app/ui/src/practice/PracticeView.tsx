// SPDX-License-Identifier: GPL-3.0-or-later
// Practice mode on the singing screen (WO2 4.5): the lane (bars, piano keys, marker) above
// the usual lyrics display, both switchable (owner decision 2), with the panels practice
// needs: the microphone explanation before the operating system asks, watch only, the
// input picker, music coming back through the mixer, practice data not ready yet, and the
// result screen. Class hooks: .practice, .practice__lane, .practice__lyrics,
// .practice__chips, .practice-chip--*, .practice-panel, .practice-tools.
import type { ComponentChildren } from "preact";
import { useState } from "preact/hooks";
import type { AppSettings, PracticeState, SongDetail } from "../api/types";
import { DEFAULT_PRACTICE_SETTINGS, LANE_BAR_COLORS, lookOf } from "../api/defaults";
import { useApp } from "../state/app";
import { useResolvedMotion } from "../skin/motion";
import {
  allowMicrophone,
  finishTake,
  openMicSettings,
  openPicker,
  setOctave,
  showPanel,
  startPractice,
  stopPractice,
  usePractice,
  watchOnly,
} from "../state/practice";
import { PageLyrics } from "../ticker/PageLyrics";
import { Ticker } from "../ticker/Ticker";
import { InputPicker } from "./InputPicker";
import { PracticeLane } from "./PracticeLane";
import { spellingFor } from "./pitch";
import { PracticeResultScreen } from "./Result";
import "./practice.css";

/** The practice settings (APP_API 12.7), defaults filled in for an older core. */
export function practiceSettings(s: AppSettings | null) {
  return { ...DEFAULT_PRACTICE_SETTINGS, ...(s?.practice ?? {}) };
}

function Panel({ title, children, tone = "info", wide = false }: { title: string; children: ComponentChildren; tone?: "info" | "warn"; wide?: boolean }) {
  return (
    <div class="practice-panel" role="dialog" aria-modal="false" aria-label={title}>
      <div class={`practice-panel__box practice-panel__box--${tone}${wide ? " practice-panel__box--wide" : ""}`}>
        <h3 class="practice-panel__title">{title}</h3>
        {children}
      </div>
    </div>
  );
}

/** Before the OS asks for the microphone: the app's own words (WO2 4.4 "Permission"). */
function PermissionPanel({ explanation }: { explanation: string }) {
  return (
    <Panel title="Practice listens to your microphone">
      <p class="practice-panel__text">{explanation}</p>
      <div class="practice-panel__actions">
        <button type="button" class="button button--primary" onClick={() => void allowMicrophone()}>
          Continue
        </button>
        <button type="button" class="button" onClick={() => void watchOnly()}>
          Watch only (no microphone)
        </button>
        <button type="button" class="button" onClick={() => void stopPractice()}>
          Not now
        </button>
      </div>
    </Panel>
  );
}

/** The mixer is sending the app's music back (WO2 4.4 / 4.10): what it does and how to stop it. */
function MusicPanel({ st }: { st: PracticeState }) {
  const m = st.music;
  const graded = m.grading === "normal" ? "Your grade is not affected." : m.grading === "less_certain" ? "Your grade will be marked as less certain." : "This take will not be graded.";
  return (
    <Panel title="The mixer is sending the music back" tone="warn" wide>
      <p class="practice-panel__text">{m.message ?? "The app's own music is coming back through the input."}</p>
      <p class="practice-panel__text">
        {m.removal === "good"
          ? `The app removes it well (by ${m.suppression_db ?? "?"} dB), and with nobody singing the marker stays off. ${graded}`
          : m.removal === "marginal"
            ? `The app can only partly remove it, so the marker may jump now and then. ${graded}`
            : m.removal === "unusable"
              ? `It comes back with a delay that keeps changing (as over Bluetooth), so it cannot be removed. The marker still shows your voice when it can. ${graded}`
              : `The app is checking whether it can remove it. ${graded}`}
      </p>
      <ol class="practice-panel__steps">
        <li>On the mixer, switch LOOPBACK off: the computer's own sound then stays out of what the mixer sends back.</li>
        <li>
          Best: play the music to the mixer over its USB cable (choose the mixer as the output in the Mixer panel) with LOOPBACK off. You hear the song in your in-ears; the app hears only your voice.
        </li>
        <li>Music reaching the mixer through AUX-IN or Bluetooth is part of what it sends to the computer: unplug it while you practise, or use step 2. Bluetooth cannot be removed at all.</li>
      </ol>
      <div class="practice-panel__actions">
        <button type="button" class="button button--primary" onClick={() => showPanel(null)}>
          Got it
        </button>
        <button type="button" class="button" onClick={() => void openPicker()}>
          Choose another input…
        </button>
      </div>
    </Panel>
  );
}

function NotReadyPanel({ st, song }: { st: PracticeState; song: SongDetail }) {
  if (st.phase === "preparing") {
    const g = st.guide;
    const pct = Math.round((g.progress ?? 0) * 100);
    return (
      <Panel title="Getting practice ready">
        <p class="practice-panel__text">
          {g.state === "queued" ? "Practice data for this song is waiting its turn…" : `Adding practice data to “${song.summary.title}” from its own lead vocal… ${pct} %`}
        </p>
        <div class="practice-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
          <span style={{ width: `${pct}%` }} />
        </div>
        {/* Measured in the real app: about 12 s for a 4-minute song on the build PC (build,
            file swap and reload), so "a second or two" promised too little. */}
        <p class="practice-panel__hint">Nothing else in the song file changes. It takes a few seconds.</p>
        <div class="practice-panel__actions">
          <button type="button" class="button" onClick={() => void stopPractice()}>
            Cancel
          </button>
        </div>
      </Panel>
    );
  }
  const nr = st.not_ready;
  return (
    <Panel title="Practice is not ready for this song" tone="warn">
      <p class="practice-panel__text">{nr?.message ?? st.guide.reason ?? "This song has no practice data yet."}</p>
      <div class="practice-panel__actions">
        {nr?.can_retry !== false && (
          <button
            type="button"
            class="button button--primary practice__add-data"
            onClick={() => {
              void stopPractice().then(() => startPractice());
            }}
          >
            {st.guide.state === "failed" ? "Try again" : "Add practice data"}
          </button>
        )}
        <button type="button" class="button" onClick={() => void stopPractice()}>
          Leave practice
        </button>
      </div>
    </Panel>
  );
}

/** Small status pills over the lane's top-right corner. */
function Chips({ st }: { st: PracticeState }) {
  const chips: ComponentChildren[] = [];
  if (st.phase === "watch_only") {
    chips.push(
      <button key="w" type="button" class="practice-chip practice-chip--watch" title={st.watch_only?.message ?? ""} onClick={() => void openPicker()}>
        Watch only
      </button>,
    );
  }
  const inp = st.input;
  if (inp) {
    const adv = inp.meter?.advice;
    const level = inp.meter ? Math.max(0, Math.min(1, (inp.meter.rms_db + 60) / 54)) : 0;
    chips.push(
      <button key="i" type="button" class={`practice-chip practice-chip--input${adv === "too_quiet" || adv === "clipping" ? " practice-chip--warn" : ""}`} title={inp.message ?? `${inp.device.name}, channel ${inp.channel + 1}`} onClick={() => void openPicker()}>
        <span class="practice-chip__meter" aria-hidden="true">
          <span style={{ transform: `scaleY(${level.toFixed(2)})` }} />
        </span>
        {adv === "too_quiet" ? "Too quiet" : adv === "clipping" ? "Clipping" : inp.state === "unplugged" ? "Input unplugged" : inp.state === "suspended" ? "Paused" : `Ch ${inp.channel + 1}`}
      </button>,
    );
  }
  if (st.music.presence === "present") {
    chips.push(
      <button key="m" type="button" class="practice-chip practice-chip--warn" onClick={() => showPanel("music")}>
        Music in input
      </button>,
    );
  }
  if (st.take?.running && st.take.overall !== null) {
    chips.push(
      <span key="s" class="practice-chip practice-chip--score" title="Running score of the notes sung so far">
        {Math.round(st.take.overall)} %
      </span>,
    );
  }
  if (chips.length === 0) return null;
  return <div class="practice__chips">{chips}</div>;
}

/** Watch only for a reason the singer can fix: say so with the exact steps. */
function WatchOnlyNote({ st }: { st: PracticeState }) {
  const w = st.watch_only;
  const perm = usePractice((s) => s.permission);
  const [hidden, setHidden] = useState<string | null>(null);
  if (!w || w.code === "chosen" || hidden === w.code) return null;
  const windowsOff = perm?.windows && (perm.windows.microphone_access === "deny" || perm.windows.desktop_apps === "deny" || perm.windows.user_access === "deny");
  return (
    <div class="practice-note" role="status">
      <p>
        <strong>{w.code === "no_device" ? "No microphone found." : w.code === "input_lost" ? "The input stopped." : windowsOff ? "Windows' microphone privacy switch is off." : "The microphone is not allowed."}</strong>{" "}
        {w.message} The notes and keys still show the melody; nothing is graded.
      </p>
      <div class="practice-note__actions">
        {(w.code === "permission_denied" || w.code === "restricted") && (
          <button type="button" class="button button--small" onClick={() => void openMicSettings()}>
            Open the privacy settings
          </button>
        )}
        <button type="button" class="button button--small" onClick={() => void openPicker()}>
          Choose input…
        </button>
        <button type="button" class="button button--small" onClick={() => setHidden(w.code)} title="The Watch only chip stays at the top">
          Hide
        </button>
      </div>
    </div>
  );
}

/** Controls in the stage's tool row while practice is on. */
export function PracticeTools() {
  const st = usePractice((s) => s.state);
  if (!st?.active) return null;
  const o = st.octave;
  const value = o.mode === "auto" ? "auto" : o.mode === "strict" ? "strict" : `fixed:${o.shift}`;
  const label = (n: number) => (n === 0 ? "As written" : `${Math.abs(n)} octave${Math.abs(n) > 1 ? "s" : ""} ${n < 0 ? "down" : "up"}`);
  return (
    <div class="practice-tools">
      <label class="practice-tools__octave">
        <span>Octave</span>
        <select
          value={value}
          onChange={(e) => {
            const v = (e.currentTarget as HTMLSelectElement).value;
            if (v === "auto" || v === "strict") void setOctave(v);
            else void setOctave("fixed", Number(v.split(":")[1]));
          }}
          title="Moves the bars and the key names to the octave you sing in. It never changes the music. Auto finds it from your first phrases; Strict requires the written octave."
        >
          <option value="auto">{o.mode === "auto" ? `Auto${o.settled ? ` (${label(o.shift).toLowerCase()})` : ""}` : "Auto"}</option>
          {[-2, -1, 0, 1, 2].map((n) => (
            <option key={n} value={`fixed:${n}`}>
              {label(n)}
            </option>
          ))}
          <option value="strict">Strict (written octave)</option>
        </select>
      </label>
      <button type="button" class="button button--small" onClick={() => void openPicker()}>
        Input…
      </button>
      {st.phase === "listening" && (
        <button type="button" class="button button--small" disabled={!st.take?.running} onClick={() => void finishTake()} title="Grade what you have sung so far">
          Finish
        </button>
      )}
    </div>
  );
}

/** The practice area: lane + lyrics, or a panel, or the result screen. */
export function PracticeView({ song, lyricsOffsetS, deviceOffsetS }: { song: SongDetail; lyricsOffsetS: number; deviceOffsetS: number }) {
  const p = usePractice((s) => s);
  const settings = useApp((s) => s.settings);
  const player = useApp((s) => s.player);
  const ui = settings?.ui;
  const motion = useResolvedMotion(ui?.motion ?? "system");
  const pr = practiceSettings(settings);
  const st = p.state;
  if (!ui) return null;
  const reduce = motion === "reduce";
  const showLyrics = pr.lyrics_below && !!song.lyrics && song.lyrics.lines.length > 0;
  const keyHeard = player?.key_heard ?? player?.key_requested ?? 0;
  const spelling = player?.key_spelling ?? spellingFor(song.summary.key, keyHeard);
  const colors = ui.colors;
  const look = lookOf(ui);

  let body: ComponentChildren;
  if (p.panel === "permission" && p.permission) body = <PermissionPanel explanation={p.permission.explanation} />;
  else if (!st || p.busy) body = <div class="practice-panel"><p class="practice-panel__wait" aria-busy="true">Starting practice…</p></div>;
  else if (st.phase === "preparing" || st.phase === "not_ready") body = <NotReadyPanel st={st} song={song} />;
  else body = null;

  const laneReady = !!st && !!p.guide && (st.phase === "listening" || st.phase === "watch_only" || st.phase === "result");
  return (
    <div class={`practice${showLyrics ? "" : " practice--lane-only"}`} data-phase={st?.phase ?? "starting"}>
      <div class="practice__lane">
        {laneReady && (
          <PracticeLane
            guide={p.guide!}
            lyrics={song.lyrics}
            lyricsOffsetS={lyricsOffsetS}
            duration={song.summary.duration_s}
            offsetS={deviceOffsetS}
            pianoSide={pr.piano_side}
            words={pr.words_under_notes}
            simple={pr.simple_lane}
            reduceMotion={reduce}
            watchOnly={st!.phase === "watch_only"}
            follow={look.watch_follow}
            celebrate={look.celebrate}
            celebration={look.celebration}
            celebrateFreq={look.celebrate_freq}
            pitchFx={look.pitch_fx}
            barColor={LANE_BAR_COLORS[look.lane_bars]}
            octaveMode={st!.octave.mode}
            spelling={spelling}
            colors={colors}
          />
        )}
        {st && laneReady && <Chips st={st} />}
        {st && laneReady && <WatchOnlyNote st={st} />}
      </div>
      {showLyrics && laneReady && (
        <div class="practice__lyrics">
          {ui.lyrics_display === "scroll" ? (
            <Ticker lyrics={song.lyrics!} offsetS={lyricsOffsetS} textSize={Math.round(ui.lyrics_text_size * 0.62)} marker={ui.marker_position} pace={ui.lyrics_motion ?? "word"} colors={ui.colors} />
          ) : (
            <PageLyrics lyrics={song.lyrics!} offsetS={lyricsOffsetS} textSize={Math.round(ui.lyrics_text_size * 0.62)} pace={ui.lyrics_motion ?? "word"} showNext={ui.show_next_line ?? true} showMarker={ui.show_marker ?? true} colors={ui.colors} />
          )}
        </div>
      )}
      {body}
      {st && p.panel === "music" && <MusicPanel st={st} />}
      {p.panel === "picker" && <InputPicker />}
      {p.result && <PracticeResultScreen result={p.result} song={song} />}
    </div>
  );
}
