// SPDX-License-Identifier: GPL-3.0-or-later
// Recorder (WORKING ORDER 4.6): one big button captures everything the computer is
// playing, losslessly, at the output's own format. Shows a level meter, elapsed time,
// the real capture format with the core's note, a clip warning, the recordings, and
// "Make a karaoke song from this" where the Studio is available.
// macOS: explains the "Screen Recording" permission BEFORE asking for it (4.10).
// Class hooks: .recorder, .recorder__button, .recorder__button--on, .recorder__meter,
// .recorder__meter-bar, .recorder__meter-fill, .recorder__elapsed, .recorder__format,
// .recorder__clip, .recorder__permission, .recordings, .recordings__item.
import { useState } from "preact/hooks";
import type { RecorderStatus } from "../api/types";
import { errorMessage } from "../api/client";
import { formatBytes, formatDb, formatRate, formatSampleFormat, formatTime } from "../lib/format";
import { addToStudio, getApiSync, refreshRecordings, store, toast, useApp } from "../state/app";
import { Note, Section } from "./common";

const METER_FLOOR_DB = -60;

function Meter({ status }: { status: RecorderStatus }) {
  const labels = status.peak_db.length === 2 ? ["L", "R"] : status.peak_db.map((_, i) => String(i + 1));
  return (
    <div class="recorder__meter" role="group" aria-label="Level">
      {status.peak_db.map((db, i) => {
        const v = Math.max(0, Math.min(1, (db - METER_FLOOR_DB) / -METER_FLOOR_DB));
        return (
          <div key={i} class="recorder__meter-row">
            <span class="recorder__meter-label">{labels[i]}</span>
            <div class="recorder__meter-bar" role="meter" aria-label={`Channel ${labels[i]} level`} aria-valuemin={METER_FLOOR_DB} aria-valuemax={0} aria-valuenow={Math.round(db)}>
              <div class={`recorder__meter-fill${db > -1 ? " recorder__meter-fill--hot" : ""}`} style={{ width: `${v * 100}%` }} />
            </div>
            <span class="recorder__meter-db">{formatDb(db)}</span>
          </div>
        );
      })}
    </div>
  );
}

/** macOS: why an audio recorder asks for "Screen Recording", shown before the system prompt. */
function MacPermission({ permission }: { permission: "denied" | "not_determined" }) {
  const [asking, setAsking] = useState(false);
  const ask = async () => {
    setAsking(true);
    try {
      await getApiSync().recorderRequestPermission();
      const info = await getApiSync().appInfo();
      store.set({ info });
    } catch (err) {
      toast(errorMessage(err), "error");
    } finally {
      setAsking(false);
    }
  };
  return (
    <div class="recorder__permission">
      <h3 class="panel__subtitle">Why macOS will ask about “Screen Recording”</h3>
      <p>
        macOS only lets an app capture the computer's sound through its screen-capture system. That is why the permission is called <strong>Screen Recording</strong>, even though this recorder keeps
        only the sound.
      </p>
      <ul>
        <li>macOS insists on a picture as well, so the app asks for the smallest one possible (2 × 2 pixels, once a second) and throws it away at once. Nothing of your screen is saved.</li>
        <li>After you allow it, macOS may ask you to quit and reopen the app.</li>
        <li>This test build is not signed by Apple, so macOS may ask again after each new version.</li>
      </ul>
      {permission === "denied" ? (
        <Note kind="warn">
          Recording is switched off for this app. Open <strong>System Settings → Privacy &amp; Security → Screen Recording</strong>, turn this app on, then quit and reopen it.
        </Note>
      ) : (
        <button type="button" class="button button--primary" disabled={asking} onClick={() => void ask()}>
          {asking ? "Waiting for macOS…" : "Continue to the macOS permission prompt"}
        </button>
      )}
    </div>
  );
}

export function Recorder() {
  const info = useApp((s) => s.info);
  const status = useApp((s) => s.recorder);
  const recordings = useApp((s) => s.recordings);
  const studioOk = useApp((s) => s.info?.capabilities.studio.available ?? false);
  const [busy, setBusy] = useState(false);
  if (!info) return null;
  const cap = info.capabilities.recorder;
  const mac = info.platform === "macos";
  const needsPermission = mac && (cap.permission === "not_determined" || cap.permission === "denied");
  const recording = status?.state === "recording" || status?.state === "starting";

  const toggle = async () => {
    setBusy(true);
    try {
      if (recording) {
        const rec = await getApiSync().recorderStop();
        toast(`Saved ${rec.name} (${formatTime(rec.duration_s)}).`);
        await refreshRecordings();
      } else {
        await getApiSync().recorderStart();
      }
      store.set({ recorder: await getApiSync().recorderStatus() });
    } catch (err) {
      toast(errorMessage(err), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="recorder">
      <Section title="Recorder">
        <p class="hint">
          Records everything this computer is playing, losslessly. It captures the system mix exactly as the output device receives it, so the quality ceiling is that device's format, shown below.
          Saved to <code>{info.paths.recordings}</code>.
        </p>
        {!cap.available && <Note kind="warn">The recorder is not available on this computer: {cap.reason ?? "no reason was given."}</Note>}
        {cap.available && needsPermission && <MacPermission permission={cap.permission as "denied" | "not_determined"} />}
        {cap.available && !needsPermission && (
          <div class="recorder__main">
            <button
              type="button"
              class={`recorder__button${recording ? " recorder__button--on" : ""}`}
              aria-pressed={recording}
              aria-label={recording ? "Stop recording" : "Start recording"}
              disabled={busy || status?.state === "stopping"}
              onClick={() => void toggle()}
            >
              <span class="recorder__button-dot" aria-hidden="true" />
              <span>{recording ? "Stop" : "Record"}</span>
            </button>
            <div class="recorder__readout">
              <p class="recorder__elapsed" aria-live="off">
                {formatTime(status?.elapsed_s ?? 0)}
              </p>
              {status && status.peak_db.length > 0 && <Meter status={status} />}
              {status?.format ? (
                <p class="recorder__format">
                  Capturing <strong>{formatRate(status.format.sample_rate)}</strong>, {status.format.channels} channel{status.format.channels === 1 ? "" : "s"}, {formatSampleFormat(status.format.sample_format)}. {status.format.note}
                </p>
              ) : (
                <p class="recorder__format">The capture format is shown once recording starts (it is whatever the output device uses).</p>
              )}
              {status?.clipped && (
                <Note kind="warn">
                  <span class="recorder__clip">Clipping:</span> the sound reached full scale while recording. Turn the playing app or the system volume down a little and record again.
                </Note>
              )}
              {status && status.dropped_s > 0 && <Note kind="warn">{status.dropped_s.toFixed(2)} s went missing during capture and were filled with silence to keep the timing right.</Note>}
              {status?.error && <Note kind="error">{status.error}</Note>}
            </div>
          </div>
        )}
      </Section>

      <Section title="Recordings" class="recordings">
        {recordings.length === 0 && <p>No recordings yet.</p>}
        <ul class="recordings__list">
          {recordings.map((r) => (
            <li key={r.path} class="recordings__item">
              <span class="recordings__name">{r.name}</span>
              <span class="recordings__meta">
                {formatTime(r.duration_s)} · {formatRate(r.sample_rate)} · {r.channels} ch · {formatBytes(r.bytes)}
              </span>
              {r.clipped && <span class="title-strip__badge title-strip__badge--warn">Clipped</span>}
              {studioOk && (
                <button type="button" class="button button--small" onClick={() => addToStudio([r.path])}>
                  Make a karaoke song from this
                </button>
              )}
            </li>
          ))}
        </ul>
      </Section>
    </div>
  );
}
