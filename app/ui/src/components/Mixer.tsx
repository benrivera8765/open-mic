// SPDX-License-Identifier: GPL-3.0-or-later
// Mixer (WORKING ORDER 4.3): backing vocals on/off and level, guide vocal 0-100 %
// (the original singer, off by default), drums and band levels, whether the drums
// follow the key, master volume, output device, exclusive mode (Windows), clip light,
// and "Export backing track". Song settings are saved into the song file by the core.
// Class hooks: .mixer, .mixer__group, .mixer__clip, .mixer__clip--on, .mixer__device,
// .mixer__export.
import { useState } from "preact/hooks";
import type { ExportFormat } from "../api/types";
import { formatRate, percent } from "../lib/format";
import { changeSongSettings, chooseDevice, exportBacking, refreshDevices, setMaster, useApp } from "../state/app";
import { Note, Progress, Section, Select, Slider, Toggle } from "./common";

function ClipLight() {
  const player = useApp((s) => s.player);
  const peaks = player?.output_peak_db;
  const clipped = player?.clipped;
  if (clipped === undefined && peaks === undefined) {
    return (
      <p class="mixer__clip mixer__clip--unknown" title="The audio engine does not report output peaks yet">
        <span class="mixer__clip-light" aria-hidden="true" /> Clip: not reported
      </p>
    );
  }
  const peak = peaks && peaks.length ? Math.max(...peaks) : -Infinity;
  return (
    <p class={`mixer__clip${clipped ? " mixer__clip--on" : ""}`} role="status">
      <span class="mixer__clip-light" aria-hidden="true" />
      {clipped ? "Clipped! Lower a level or the master volume." : `No clipping${Number.isFinite(peak) && peak > -120 ? ` (peak ${peak.toFixed(1)} dBFS)` : ""}`}
    </p>
  );
}

function OutputDevice() {
  const devices = useApp((s) => s.devices);
  const settings = useApp((s) => s.settings);
  const info = useApp((s) => s.info);
  const dev = useApp((s) => s.player?.device ?? null);
  const underruns = useApp((s) => (s.player?.underruns ?? 0) + (s.player?.device_starved ?? 0));
  if (!settings) return null;
  const selected = settings.audio.device_id ?? "";
  const selDev = devices.find((d) => d.id === selected) ?? devices.find((d) => d.is_default);
  const canExclusive = info?.platform === "windows" && info.capabilities.exclusive_mode && (selDev?.exclusive_capable ?? false);
  return (
    <div class="mixer__device">
      <Select<string>
        label="Output"
        value={selected}
        options={[{ value: "", label: "System default" }, ...devices.map((d) => ({ value: d.id, label: `${d.name} (${formatRate(d.sample_rate)})` }))]}
        onChange={(v) => void chooseDevice(v || null, settings.audio.exclusive)}
      />
      <button type="button" class="button button--small" onClick={() => void refreshDevices()}>
        Refresh list
      </button>
      {info?.platform === "windows" && (
        <Toggle
          label="Exclusive mode"
          checked={settings.audio.exclusive}
          disabled={!canExclusive}
          hint={canExclusive ? "Bypasses the Windows mixer; other apps go silent while a song plays." : "This device does not support exclusive mode."}
          onChange={(v) => void chooseDevice(settings.audio.device_id, v)}
        />
      )}
      {dev && (
        <p class="mixer__format">
          Playing on <strong>{dev.name}</strong>: {formatRate(dev.sample_rate)}, {dev.channels} ch{dev.exclusive ? ", exclusive" : ", shared"}, output latency {Math.round(dev.latency_ms)} ms
          {underruns > 0 && <span class="mixer__underruns"> · {underruns} dropout{underruns === 1 ? "" : "s"}</span>}
        </p>
      )}
    </div>
  );
}

function ExportBacking() {
  const song = useApp((s) => s.song);
  const exports = useApp((s) => s.exports);
  const [format, setFormat] = useState<ExportFormat>("flac24");
  const jobs = Object.entries(exports);
  return (
    <div class="mixer__export">
      <h3 class="panel__subtitle">Export backing track</h3>
      <p class="hint">Renders the current key and mix (guide vocal included at its level) to a lossless file in the Exports folder.</p>
      <div class="row">
        <Select<ExportFormat>
          label="Format"
          value={format}
          options={[
            { value: "flac24", label: "FLAC, 24-bit" },
            { value: "wav32f", label: "WAV, 32-bit float" },
          ]}
          onChange={setFormat}
        />
        <button type="button" class="button" disabled={!song} onClick={() => void exportBacking(format)}>
          Export
        </button>
      </div>
      {jobs.map(([id, j]) => (
        <div key={id} class="mixer__export-job">
          {j.error ? <Note kind="error">{j.error}</Note> : j.path ? <Note>Saved: {j.path}</Note> : <Progress value={j.progress} label="Export progress" />}
        </div>
      ))}
    </div>
  );
}

export function Mixer() {
  const song = useApp((s) => s.song);
  const st = useApp((s) => s.songSettings);
  const master = useApp((s) => s.settings?.audio.master_volume ?? 1);
  const roles = new Set(song?.stems.map((s) => s.role) ?? []);
  const off = !song || !st;
  const lv = (key: "drums" | "other") => (v: number) => changeSongSettings({ levels: { ...(st?.levels ?? { drums: 1, other: 1 }), [key]: v } }, 40);

  return (
    <Section title="Mixer" class="mixer">
      <div class="mixer__group">
        <Toggle label="Backing vocals" checked={st?.backing_on ?? true} disabled={off || !roles.has("backing_vocals")} onChange={(v) => changeSongSettings({ backing_on: v })} />
        <Slider label="Backing level" value={st?.backing_level ?? 1} min={0} max={1} step={0.01} format={percent} disabled={off || !roles.has("backing_vocals") || !st?.backing_on} onInput={(v) => changeSongSettings({ backing_level: v }, 40)} />
        <Slider label="Guide vocal (original singer)" value={st?.guide_vocal ?? 0} min={0} max={1} step={0.01} format={percent} disabled={off || !roles.has("lead_vocal")} onInput={(v) => changeSongSettings({ guide_vocal: v }, 40)} />
        <Slider label="Drums" value={st?.levels.drums ?? 1} min={0} max={1} step={0.01} format={percent} disabled={off || !roles.has("drums")} onInput={lv("drums")} />
        <Slider label="Band (everything else)" value={st?.levels.other ?? 1} min={0} max={1} step={0.01} format={percent} disabled={off || !roles.has("other")} onInput={lv("other")} />
        <Toggle
          label="Shift drums with the key"
          checked={st?.shift_drums ?? false}
          disabled={off}
          hint="Off keeps the drums untouched, which sounds more natural."
          onChange={(v) => changeSongSettings({ shift_drums: v })}
        />
      </div>
      <div class="mixer__group">
        <Slider label="Master volume" value={master} min={0} max={1} step={0.01} format={percent} onInput={(v) => void setMaster(v)} />
        <ClipLight />
      </div>
      <OutputDevice />
      <ExportBacking />
    </Section>
  );
}
