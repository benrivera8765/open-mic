// SPDX-License-Identifier: GPL-3.0-or-later
// Recorder (WORKING ORDER 4.6): one big button captures everything the computer is
// playing, losslessly, at the output's own format. Shows a level meter, elapsed time,
// the real capture format with the core's note, a clip warning, the recordings, and
// "Make a karaoke song from this" where the Studio is available.
// macOS: explains the "Screen Recording" permission BEFORE asking for it (4.10).
// Class hooks: .recorder, .recorder__button, .recorder__button--on, .recorder__meter,
// .recorder__meter-bar, .recorder__meter-fill, .recorder__elapsed, .recorder__format,
// .recorder__clip, .recorder__permission, .recorder__honesty, .recordings, .recordings__item.
// Honest about the capture (4.6): Windows' own processing ("Audio enhancements") is in
// the mix; on the Mac the format shown is what ScreenCaptureKit delivers.
//
// The owner's request of 18 Nov 2026 (DECISIONS D-221..): the music player can be
// started together with the capture ("Record & play": capture first, then play, so the
// first note is never missed), and several songs can be recorded back to back and kept
// apart ("Record songs separately": one capture, cut into one file per song when it
// stops, ready for the Studio). Windows only (the system's media sessions).
// More hooks: .recorder__player, .recorder__modes, .recorder__mode-button,
// .recorder__options, .recorder__live, .recorder__player-notes, .rec-sessions,
// .rec-session, .rec-song, .rec-song__badge--partial, .rec-song__cover.
import { useEffect, useState } from "preact/hooks";
import type { MediaPlayer, RecordedSong, RecorderStatus, RecordingSession } from "../api/types";
import { errorMessage } from "../api/client";
import { formatBytes, formatDb, formatRate, formatSampleFormat, formatTime } from "../lib/format";
import {
  addToStudio,
  getApiSync,
  makeKaraokeSongs,
  refreshPlayers,
  refreshRecordings,
  refreshRecordingSessions,
  store,
  toast,
  togglePreview,
  useApp,
} from "../state/app";
import { Note, Progress, Section, useId } from "./common";

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

// ---------------------------------------------------------------------------
// Remembered choices (per computer; the page works without storage)
// ---------------------------------------------------------------------------

function remembered(key: string, fallback: string): string {
  try {
    return globalThis.localStorage?.getItem(`jk.recorder.${key}`) ?? fallback;
  } catch {
    return fallback;
  }
}
function remember(key: string, value: string): void {
  try {
    globalThis.localStorage?.setItem(`jk.recorder.${key}`, value);
  } catch {
    // Storage blocked: the choice lasts until the window closes.
  }
}

function playerLabel(p: MediaPlayer): string {
  const what = [p.artist, p.title].filter((x) => x.trim()).join(" – ");
  const state = p.status === "playing" ? "playing" : p.status === "paused" ? "paused" : p.status;
  return what ? `${p.app_name}: ${what} (${state})` : `${p.app_name} (${state}, nothing loaded)`;
}

/** The player to suggest: the one playing, else the one Windows calls current, else the first. */
function suggestedPlayer(players: MediaPlayer[]): string {
  return (players.find((p) => p.status === "playing") ?? players.find((p) => p.is_current) ?? players[0])?.id ?? "";
}

// ---------------------------------------------------------------------------
// Recorded songs
// ---------------------------------------------------------------------------

/** A cover thumbnail (a JPEG / PNG next to the song), or an empty square. */
function CoverThumb({ path }: { path: string | null }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    const api = getApiSync();
    if (!path || !api.studioCoverPreview) return;
    let cancelled = false;
    api
      .studioCoverPreview(path, 96)
      .then((r) => !cancelled && setUrl(r.data_url))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [path]);
  return url ? <img class="rec-song__cover" src={url} alt="" width={48} height={48} /> : <span class="rec-song__cover rec-song__cover--none" aria-hidden="true" />;
}

const STATUS_TEXT: Record<RecordedSong["status"], string> = {
  complete: "Complete",
  partial: "Partial",
  unknown: "Length not reported",
};

function cutHint(song: RecordedSong): string | null {
  const ends = [song.start_confidence === "uncertain" ? "start" : null, song.end_confidence === "uncertain" ? "end" : null].filter(Boolean);
  if (ends.length === 0) return null;
  return `Check the ${ends.join(" and ")}: there was no silence between the songs there, so it was cut where the player said the song changed.`;
}

function PreviewButton({ path, label }: { path: string; label: string }) {
  const playing = useApp((s) => s.previewPath === path);
  return (
    <button type="button" class="button button--small rec-song__preview" aria-pressed={playing} aria-label={`${playing ? "Stop" : "Play"} ${label}`} onClick={() => void togglePreview(path)}>
      {playing ? "■ Stop" : "▶ Play"}
    </button>
  );
}

function SongRow({ song, studioOk }: { song: RecordedSong; studioOk: boolean }) {
  const hint = cutHint(song);
  const label = [song.artist, song.title].filter((x) => x.trim()).join(" – ") || song.name;
  return (
    <li class={`rec-song rec-song--${song.status}`}>
      <CoverThumb path={song.cover_path} />
      <div class="rec-song__text">
        <span class="rec-song__title">
          {String(song.number).padStart(2, "0")} {song.title || "Unknown title"}
        </span>
        <span class="rec-song__meta">
          {song.artist || "Unknown artist"}
          {song.album ? ` · ${song.album}` : ""} · {formatTime(song.duration_s)}
          {song.track_duration_s ? ` of ${formatTime(song.track_duration_s)}` : ""}
        </span>
        {song.partial_reason && <span class="rec-song__reason">{song.partial_reason}</span>}
        {hint && <span class="rec-song__reason">{hint}</span>}
        {song.paused_s >= 1 && <span class="rec-song__reason">Includes {formatTime(song.paused_s)} while the player was paused.</span>}
      </div>
      <span class={`title-strip__badge rec-song__badge rec-song__badge--${song.status}${song.status === "partial" ? " title-strip__badge--warn" : ""}`}>{STATUS_TEXT[song.status]}</span>
      <PreviewButton path={song.path} label={label} />
      {studioOk && (
        <button type="button" class="button button--small" onClick={() => void makeKaraokeSongs([song])}>
          Make a karaoke song
        </button>
      )}
    </li>
  );
}

const STATE_TEXT: Record<RecordingSession["state"], string> = {
  recording: "Recording…",
  stopped: "Waiting to be split into songs",
  splitting: "Splitting into songs…",
  split: "Split into songs",
  kept_master: "Split, but the full recording was kept",
  failed: "Not split",
  interrupted: "Interrupted: it is split when the app starts next",
};

const STOP_TEXT: Record<string, string> = {
  user: "stopped with Stop",
  idle: "stopped by itself after the player was paused",
  max_songs: "stopped by itself after the song limit",
  player_closed: "stopped by itself: the player was closed",
  interrupted: "the app closed while recording; everything up to then was kept",
  app_closed: "the app was closed while recording",
};

function SessionItem({ s, studioOk }: { s: RecordingSession; studioOk: boolean }) {
  const [busy, setBusy] = useState(false);
  const complete = s.songs.filter((x) => x.status === "complete");
  const canSplit = s.master_present && (s.state === "kept_master" || s.state === "failed" || s.state === "stopped");
  const makeAll = async () => {
    setBusy(true);
    try {
      await makeKaraokeSongs(complete);
    } finally {
      setBusy(false);
    }
  };
  const splitAgain = async () => {
    setBusy(true);
    try {
      await getApiSync().recorderSessionSplit?.(s.folder);
    } catch (err) {
      toast(errorMessage(err), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <li class={`rec-session rec-session--${s.state}`} aria-label={`Session ${s.created}`}>
      <div class="rec-session__header">
        <strong class="rec-session__title">{s.created}</strong>
        <span class="rec-session__meta">
          from {s.player} · {STATE_TEXT[s.state]}
          {s.state === "split" || s.state === "kept_master" ? ` · ${s.songs.length} song${s.songs.length === 1 ? "" : "s"}` : ""}
          {s.stop_reason && STOP_TEXT[s.stop_reason] ? ` · ${STOP_TEXT[s.stop_reason]}` : ""}
        </span>
      </div>
      {s.state === "splitting" && <Progress value={s.progress ?? 0} label={`Splitting ${s.created}`} class="rec-session__progress" />}
      {s.message && <Note kind={s.state === "failed" ? "error" : "warn"}>{s.message}</Note>}
      <div class="row rec-session__actions">
        {studioOk && complete.length > 0 && (
          <button type="button" class="button button--primary button--small rec-session__make-all" disabled={busy} onClick={() => void makeAll()}>
            Make karaoke songs from all complete songs ({complete.length})
          </button>
        )}
        {canSplit && (
          <button type="button" class="button button--small" disabled={busy} onClick={() => void splitAgain()}>
            Split again
          </button>
        )}
      </div>
      {s.songs.length > 0 && (
        <ul class="rec-session__songs">
          {s.songs.map((song) => (
            <SongRow key={song.path} song={song} studioOk={studioOk} />
          ))}
        </ul>
      )}
      {s.excluded.length > 0 && (
        <details class="rec-session__excluded">
          <summary>
            {s.excluded.length} part{s.excluded.length === 1 ? "" : "s"} not used as songs (kept in the “_excluded” folder)
          </summary>
          <ul>
            {s.excluded.map((p) => (
              <li key={p.path} class="rec-session__part">
                <span>
                  {p.reason} · {formatTime(p.duration_s)}
                </span>
                <PreviewButton path={p.path} label={p.name} />
              </li>
            ))}
          </ul>
        </details>
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------------

export function Recorder() {
  const info = useApp((s) => s.info);
  const status = useApp((s) => s.recorder);
  const recordings = useApp((s) => s.recordings);
  const players = useApp((s) => s.players);
  const sessions = useApp((s) => s.recordingSessions);
  const studioOk = useApp((s) => s.info?.capabilities.studio.available ?? false);
  const [busy, setBusy] = useState(false);
  const [playerId, setPlayerId] = useState("");
  const [idleS, setIdleS] = useState(() => remembered("idle", "30"));
  const [maxSongs, setMaxSongs] = useState(() => remembered("maxSongs", ""));
  const [pauseOnStop, setPauseOnStop] = useState(() => remembered("pause", "1") === "1");
  const idleId = useId("rec-idle");
  const maxId = useId("rec-max");
  const playerSelId = useId("rec-player");
  const pauseId = useId("rec-pause");
  const recording = status?.state === "recording" || status?.state === "starting";
  const mode = recording ? (status?.mode ?? "plain") : "plain";
  const windows = info?.platform === "windows";
  const playerLink = windows && !!getApiSync().recorderMediaSessions;

  // Which players are open changes while the screen is up: ask every few seconds.
  useEffect(() => {
    if (!playerLink) return;
    void refreshPlayers();
    void refreshRecordingSessions();
    const h = setInterval(() => void refreshPlayers(), 2500);
    return () => clearInterval(h);
  }, [playerLink]);

  const list = players?.available ? players.players : [];
  const chosen = list.find((p) => p.id === playerId) ?? list.find((p) => p.id === suggestedPlayer(list)) ?? null;

  if (!info) return null;
  const cap = info.capabilities.recorder;
  const mac = info.platform === "macos";
  const needsPermission = mac && (cap.permission === "not_determined" || cap.permission === "denied");

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
      store.set({ recorder: await getApiSync().recorderStatus() });
    } catch (err) {
      toast(errorMessage(err), "error");
    } finally {
      setBusy(false);
    }
  };

  const stop = () =>
    run(async () => {
      const api = getApiSync();
      if (mode === "session" && api.recorderSessionStop) {
        const s = await api.recorderSessionStop(pauseOnStop);
        toast(`Stopped after ${formatTime(status?.elapsed_s ?? 0)}. Splitting ${s.created} into songs…`);
        await refreshRecordingSessions();
        return;
      }
      const rec = await api.recorderStop(mode === "record_and_play" && pauseOnStop);
      toast(`Saved ${rec.name} (${formatTime(rec.duration_s)}).`);
      if (rec.note) toast(rec.note, "error");
      await refreshRecordings();
    });

  const startPlain = () => run(() => getApiSync().recorderStart());

  const startRecordAndPlay = () =>
    run(async () => {
      if (!chosen) return;
      const st = await getApiSync().recorderRecordAndPlay!(chosen.id);
      if (st.note) toast(st.note, "error");
    });

  const startSession = () =>
    run(async () => {
      if (!chosen) return;
      const idle = Math.max(0, Math.min(3600, Number(idleS) || 0));
      const max = Math.floor(Number(maxSongs));
      const st = await getApiSync().recorderSessionStart!({
        player_id: chosen.id,
        auto_stop_idle_s: idle,
        max_songs: max > 0 ? max : null,
        start_player: true,
        pause_player_at_end: pauseOnStop,
      });
      if (st.note) toast(st.note, "error");
      await refreshRecordingSessions();
    });

  const live = status?.session ?? null;

  return (
    <div class="recorder">
      <Section title="Recorder">
        <p class="hint">
          Records everything this computer is playing, losslessly. It captures the system mix exactly as the output device receives it, so the quality ceiling is that device's format, shown below.
          Saved to <code>{info.paths.recordings}</code>.
        </p>
        <p class="hint recorder__honesty">
          {mac
            ? "On a Mac, the format shown is the one macOS's capture service (ScreenCaptureKit) delivers; set the output to 48,000 Hz in Audio MIDI Setup to avoid a sample-rate conversion."
            : "The mix is captured after Windows' own processing, so sound-driver effects such as “Audio enhancements” are in the recording: for an exact copy, turn them off for the output device (Settings → System → Sound → your speakers or headphones)."}
        </p>
        {!cap.available && <Note kind="warn">The recorder is not available on this computer: {cap.reason ?? "no reason was given."}</Note>}
        {cap.available && needsPermission && <MacPermission permission={cap.permission as "denied" | "not_determined"} />}
        {cap.available && !needsPermission && (
          <>
            {playerLink && (
              <div class="recorder__player">
                {!recording && players && !players.available && <Note kind="warn">The music player cannot be started from here: {players.reason ?? "Windows did not say why."}</Note>}
                {!recording && players?.available && list.length === 0 && (
                  <Note kind="info">
                    No music player is open. Open your player and start a song once (Windows lists a player only after it has played something), or use <strong>Record</strong> and press play yourself.
                  </Note>
                )}
                {!recording && list.length > 0 && (
                  <div class="field field--select recorder__player-choice">
                    <label for={playerSelId} class="field__label">
                      Music player
                    </label>
                    <select id={playerSelId} value={chosen?.id ?? ""} onChange={(e) => setPlayerId((e.currentTarget as HTMLSelectElement).value)}>
                      {list.map((p) => (
                        <option key={p.id} value={p.id}>
                          {playerLabel(p)}
                        </option>
                      ))}
                    </select>
                  </div>
                )}
              </div>
            )}
            <div class="recorder__main">
              <button
                type="button"
                class={`recorder__button${recording ? " recorder__button--on" : ""}`}
                aria-pressed={recording}
                aria-label={recording ? "Stop recording" : "Start recording"}
                disabled={busy || status?.state === "stopping"}
                onClick={() => void (recording ? stop() : startPlain())}
              >
                <span class="recorder__button-dot" aria-hidden="true" />
                <span>{recording ? "Stop" : "Record"}</span>
              </button>
              <div class="recorder__readout">
                <p class="recorder__elapsed" aria-live="off">
                  {formatTime(status?.elapsed_s ?? 0)}
                </p>
                {recording && mode !== "plain" && status?.player && (
                  <div class="recorder__live" aria-live="polite">
                    {mode === "record_and_play" && (
                      <p>
                        Recording, and <strong>{status.player.app_name}</strong> was told to play.
                      </p>
                    )}
                    {mode === "session" && live && (
                      <>
                        <p>
                          Recording songs separately from <strong>{status.player.app_name}</strong>: song <strong class="recorder__song-count">{live.songs}</strong>
                          {live.max_songs ? ` of ${live.max_songs}` : ""}
                          {live.current ? (
                            <>
                              {" "}
                              — <span class="recorder__current">{[live.current.artist, live.current.title].filter((x) => x.trim()).join(" – ") || "untitled"}</span>
                            </>
                          ) : null}
                          .
                        </p>
                        {live.auto_stop_in_s !== null && (
                          <p class="hint">
                            The player is {live.playback}; recording stops by itself in {Math.ceil(live.auto_stop_in_s)} s unless it plays again.
                          </p>
                        )}
                      </>
                    )}
                  </div>
                )}
                {status?.note && <Note kind="warn">{status.note}</Note>}
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
            {playerLink && (
              <div class="recorder__modes">
                {!recording && (
                  <div class="row">
                    <button type="button" class="button recorder__mode-button" disabled={busy || !chosen} onClick={() => void startRecordAndPlay()}>
                      Record &amp; play
                    </button>
                    <button type="button" class="button recorder__mode-button" disabled={busy || !chosen} onClick={() => void startSession()}>
                      Record songs separately
                    </button>
                  </div>
                )}
                {!recording && (
                  <div class="recorder__options">
                    <div class="field">
                      <label for={idleId} class="field__label">
                        Songs separately: stop by itself when the player is paused for (seconds, 0 = never)
                      </label>
                      <input
                        id={idleId}
                        type="number"
                        min={0}
                        max={3600}
                        step={5}
                        value={idleS}
                        onInput={(e) => {
                          const v = (e.currentTarget as HTMLInputElement).value;
                          setIdleS(v);
                          remember("idle", v);
                        }}
                      />
                    </div>
                    <div class="field">
                      <label for={maxId} class="field__label">
                        Stop after this many songs (empty = no limit)
                      </label>
                      <input
                        id={maxId}
                        type="number"
                        min={1}
                        max={999}
                        step={1}
                        value={maxSongs}
                        onInput={(e) => {
                          const v = (e.currentTarget as HTMLInputElement).value;
                          setMaxSongs(v);
                          remember("maxSongs", v);
                        }}
                      />
                    </div>
                  </div>
                )}
                <div class="field field--toggle">
                  <input
                    id={pauseId}
                    type="checkbox"
                    checked={pauseOnStop}
                    onChange={(e) => {
                      const v = (e.currentTarget as HTMLInputElement).checked;
                      setPauseOnStop(v);
                      remember("pause", v ? "1" : "0");
                    }}
                  />
                  <label for={pauseId} class="field__label">
                    Pause the player when the recording stops
                  </label>
                </div>
                <ul class="hint recorder__player-notes">
                  <li>
                    <strong>Record &amp; play</strong> starts the recording first and then tells the player to play, so the start of the song is never missed. <strong>Record songs separately</strong> keeps
                    recording while the player goes from song to song, then cuts the recording into one file per song (in the quiet between songs), named after what the player shows.
                  </li>
                  <li>The recording is the computer's sound as Windows mixes it, so the player's volume and the Windows volume are in it: set them where you want them before you start, and keep other sounds quiet.</li>
                  <li>For clean cuts between songs, switch off the player's crossfade and automix. Without a quiet gap a song is cut where the player says it changed, and is marked “check the start/end”.</li>
                  <li>Advertisements are kept out of the song list (they go to an “_excluded” folder). A song you skip, or one the player was already part-way into when you started, is marked partial.</li>
                  <li>This works on Windows only. Many streaming services' terms do not allow recording their streams: record only what you may, for your own use.</li>
                </ul>
              </div>
            )}
            {!windows && (
              <p class="hint recorder__player-notes">Starting the music player from here and recording songs separately work on Windows only for now: start the song in your player, then press Record.</p>
            )}
          </>
        )}
      </Section>

      {playerLink && (
        <Section title="Songs recorded separately" class="rec-sessions">
          {sessions.length === 0 && <p>None yet. Press “Record songs separately”, play an album or a playlist, and the songs appear here, one file each.</p>}
          <ul class="rec-sessions__list">
            {sessions.map((s) => (
              <SessionItem key={s.folder} s={s} studioOk={studioOk} />
            ))}
          </ul>
        </Section>
      )}

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
