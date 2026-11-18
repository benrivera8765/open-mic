// SPDX-License-Identifier: GPL-3.0-or-later
// Settings: look (skin, bubbles, motion), lyrics (display, next line, motion, text size,
// marker position, colours), timing offsets per output device, and information about
// this computer.
// Saved by the core into <root>/Settings/settings.json.
import type { KeyModeSetting, LyricsDisplay, LyricsMotion, MotionSetting } from "../api/types";
import { DEFAULT_APP_SETTINGS } from "../api/defaults";
import { formatOffset } from "../lib/format";
import { changeSettings, deviceOffsetKey, setOverlay, useApp } from "../state/app";
import { Section, Select, Slider, Toggle, useId } from "./common";

function ColorField({ label, value, onChange, disabled }: { label: string; value: string; onChange: (v: string) => void; disabled?: boolean }) {
  const id = useId("color");
  return (
    <div class="field field--color">
      <label for={id} class="field__label">
        {label}
      </label>
      <input id={id} type="color" value={value} disabled={disabled} onInput={(e) => onChange((e.currentTarget as HTMLInputElement).value)} />
      <code class="field__value">{value}</code>
    </div>
  );
}

export function Settings() {
  const settings = useApp((s) => s.settings);
  const info = useApp((s) => s.info);
  const devices = useApp((s) => s.devices);
  const keyMode = useApp((s) => s.player?.key_mode);
  const keyNote = useApp((s) => s.player?.key_note ?? null);
  if (!settings || !info) return null;
  const ui = settings.ui;
  const offsets = settings.audio.device_offsets_ms;
  const currentKey = deviceOffsetKey(settings);
  const deviceName = (key: string) => (key === "default" ? "System default output" : (devices.find((d) => d.id === key)?.name ?? key));
  const offsetKeys = [...new Set([currentKey, ...Object.keys(offsets)])];
  /** The page display (static lines) has no marker. */
  const page = (ui.lyrics_display ?? "page") === "page";

  return (
    <div class="settings">
      <Section title="Look">
        <Toggle label="Jukebox look" checked={ui.skin} onChange={(v) => changeSettings({ ui: { skin: v } })} hint="Off shows this plain layout." />
        <Toggle label="Bubbles" checked={ui.bubbles} disabled={!ui.skin} onChange={(v) => changeSettings({ ui: { bubbles: v } })} hint="Bubbles never run when motion is reduced." />
        <Select<MotionSetting>
          label="Motion"
          value={ui.motion}
          options={[
            { value: "system", label: "Follow the system setting" },
            { value: "reduce", label: "Reduce motion" },
            { value: "full", label: "Full motion" },
          ]}
          onChange={(v) => changeSettings({ ui: { motion: v } })}
        />
        <p class="hint">The lyrics always show where the song is (the colour wipe, or the moving ticker); reducing motion turns off the decorative animation.</p>
      </Section>

      <Section title="Lyrics">
        <Select<LyricsDisplay>
          label="Lyrics display"
          value={page ? "page" : "scroll"}
          options={[
            { value: "page", label: "Static lines with a colour wipe" },
            { value: "scroll", label: "Scrolling ticker" },
          ]}
          onChange={(v) => changeSettings({ ui: { lyrics_display: v } })}
        />
        <p class="hint">Static lines: each line stands still and the sung colour sweeps through it word by word; a small bar before a line runs out when it is time to sing. Scrolling ticker: one line of text slides under a fixed marker.</p>
        <Toggle
          label="Show the next line"
          checked={ui.show_next_line ?? true}
          disabled={!page}
          onChange={(v) => changeSettings({ ui: { show_next_line: v } })}
          hint={page ? "Smaller, under the line being sung, so you can read ahead." : "Static lines only (the ticker shows what is coming by itself)."}
        />
        <Select<LyricsMotion>
          label="Lyrics motion"
          value={ui.lyrics_motion ?? "word"}
          options={[
            { value: "word", label: "Smooth, paced by word" },
            { value: "letter", label: "Exact, letter by letter" },
          ]}
          onChange={(v) => changeSettings({ ui: { lyrics_motion: v } })}
        />
        <p class="hint">For both displays: how the colour wipe moves through a line, or how the ticker moves under the marker. Smooth: it never stops while a line is sung, slower for a held word and faster for quick ones, and each word is reached as it is sung. Exact: every letter is reached exactly while it is sung, which makes it speed up and slow down within words.</p>
        <Slider label="Text size" value={ui.lyrics_text_size} min={28} max={120} step={2} format={(v) => `${v} px`} onInput={(v) => changeSettings({ ui: { lyrics_text_size: v } }, 300)} />
        <p class="hint">Measured on a 1440 × 900 window and scaled with the window. Static lines: a line too long for the screen is made smaller or split over two rows. Ticker: if a fast passage would not leave at least 2 seconds of upcoming words on screen, the text is made smaller for that song.</p>
        <Slider
          label="Marker position (from the left)"
          value={Math.round(ui.marker_position * 100)}
          min={15}
          max={60}
          step={1}
          disabled={page}
          format={(v) => (page ? `${v} % (ticker only)` : `${v} %`)}
          onInput={(v) => changeSettings({ ui: { marker_position: v / 100 } }, 300)}
        />
        <div class="settings__colors">
          <ColorField label="Sung" value={ui.colors.sung} onChange={(v) => changeSettings({ ui: { colors: { sung: v } } }, 300)} />
          <ColorField label="Coming up" value={ui.colors.upcoming} onChange={(v) => changeSettings({ ui: { colors: { upcoming: v } } }, 300)} />
          <ColorField label="Marker" value={ui.colors.marker} disabled={page} onChange={(v) => changeSettings({ ui: { colors: { marker: v } } }, 300)} />
          <button type="button" class="button button--small" onClick={() => changeSettings({ ui: { colors: { ...DEFAULT_APP_SETTINGS.ui.colors } } })}>
            Default colours
          </button>
        </div>
      </Section>

      <Section title="Key change">
        <Select<KeyModeSetting>
          label="How the key is changed"
          value={settings.audio.key_mode ?? "auto"}
          options={[
            { value: "auto", label: "Automatic (render ahead on slower computers)" },
            { value: "realtime", label: "In real time (heard at once)" },
            { value: "prerender", label: "Render ahead and cache (never stutters)" },
          ]}
          onChange={(v) => changeSettings({ audio: { key_mode: v } })}
        />
        <p class="hint">
          Rendering ahead keeps the previous key playing for a few seconds while the new one is prepared, then switches without a click. Automatic measures this computer once and renders ahead when it is too slow to shift in real time.
          {keyMode ? ` In use now: ${keyMode === "prerender" ? "render ahead" : "real time"}.` : ""}
        </p>
        {keyNote && <p class="hint">{keyNote}</p>}
        <Slider
          label="Key cache size"
          value={settings.audio.key_cache_mb ?? 1024}
          min={256}
          max={8192}
          step={256}
          format={(v) => (v >= 1024 ? `${(v / 1024).toFixed(v % 1024 ? 2 : 0)} GB` : `${v} MB`)}
          onInput={(v) => changeSettings({ audio: { key_cache_mb: v } }, 300)}
        />
        <p class="hint">Rendered keys are kept in the Settings folder (cache/keys) and the oldest are removed beyond this size. About 90 MB per key for a 4-minute song.</p>
      </Section>

      <Section title="Timing offsets per output">
        <p class="hint">Bluetooth headphones and speakers play late. Set the delay once per output; it applies to every song. Each song also has its own offset in “Fix lyrics timing”.</p>
        {offsetKeys.map((key) => (
          <Slider
            key={key}
            label={`${deviceName(key)}${key === currentKey ? " (in use)" : ""}`}
            value={offsets[key] ?? 0}
            min={0}
            max={1000}
            step={10}
            format={formatOffset}
            onInput={(v) => changeSettings({ audio: { device_offsets_ms: { [key]: v } } }, 300)}
          />
        ))}
      </Section>

      <Section title="This computer">
        <dl class="facts">
          <dt>App</dt>
          <dd>
            {info.name} {info.version}
          </dd>
          <dt>System</dt>
          <dd>{info.os_version}</dd>
          <dt>Mode</dt>
          <dd>{info.portable ? `Portable: everything stays in ${info.root ?? "the app's folder"}` : "Installed"}</dd>
          <dt>Library</dt>
          <dd>
            <code>{info.paths.library}</code>
          </dd>
          <dt>Recordings</dt>
          <dd>
            <code>{info.paths.recordings}</code>
          </dd>
          <dt>Exports</dt>
          <dd>
            <code>{info.paths.exports}</code>
          </dd>
          <dt>Settings</dt>
          <dd>
            <code>{info.paths.settings}</code>
          </dd>
          <dt>Studio</dt>
          <dd>{info.capabilities.studio.available ? `Available (${info.capabilities.studio.gpu ?? "GPU"})` : `Not available: ${info.capabilities.studio.reasons.join(" ")}`}</dd>
          <dt>Recorder</dt>
          <dd>
            {info.capabilities.recorder.available ? "Available" : `Not available: ${info.capabilities.recorder.reason ?? ""}`}
            {info.capabilities.recorder.permission !== "not_required" ? ` (permission: ${info.capabilities.recorder.permission.replace("_", " ")})` : ""}
          </dd>
          <dt>Web engine</dt>
          <dd>
            <code>{navigator.userAgent}</code>
          </dd>
        </dl>
        {info.translocated && (
          <p class="note note--warn">
            macOS is running this app from a temporary copy, so it cannot see the folder it came from. Move the app out of Downloads, or follow the first-launch steps in the manual.
          </p>
        )}
        <button type="button" class="button button--primary" onClick={() => setOverlay("selfcheck")}>
          Run the self-check
        </button>
      </Section>
    </div>
  );
}
