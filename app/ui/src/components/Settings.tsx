// SPDX-License-Identifier: GPL-3.0-or-later
// Settings: look (skin, bubbles, motion), lyrics (display, next line, progress bar,
// motion, text size, marker position, colours), practice mode (WO2: where the piano keys
// sit, the words and lyrics, the simple lane, how strict, the octave, the timing), timing
// offsets per output device, and information about this computer.
// Saved by the core into <root>/Settings/settings.json.
import type { KeyModeSetting, LookSettings, LyricsDisplay, LyricsMotion, MotionSetting, OctaveMode, RigCheckReport, RigCheckStep, Strictness } from "../api/types";
import { errorMessage } from "../api/client";
import { DEFAULT_APP_SETTINGS, DEFAULT_PRACTICE_SETTINGS, lookOf, type Theme, THEMES, themeState } from "../api/defaults";
import { setLatency } from "../state/practice";
import { useEffect, useState } from "preact/hooks";
import { formatOffset } from "../lib/format";
import { changeSettings, deviceOffsetKey, getApiSync, setOverlay, useApp } from "../state/app";
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

/**
 * The practice rig check (WO2 4.8, APP_API 10.2): the one check that MAKES A SOUND, so it is
 * started by hand only, after the owner confirmed with the in-ears on. It plays three short
 * test bursts through the current output, listens to the input and writes what came back
 * into a report next to the self-check reports.
 */
function RigCheck() {
  const api = getApiSync();
  const [stage, setStage] = useState<"idle" | "confirm" | "running" | "done">("idle");
  const [label, setLabel] = useState("");
  const [step, setStep] = useState<RigCheckStep | null>(null);
  const [report, setReport] = useState<RigCheckReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => api.on("practice://rigcheck", (s) => setStep(s)), [api]);
  if (!api.practiceRigCheck) return null;
  const start = async () => {
    setStage("running");
    setError(null);
    setReport(null);
    try {
      setReport(await api.practiceRigCheck!({ audible_confirmed: true, label: label.trim() || undefined }));
    } catch (err) {
      setError(errorMessage(err));
    }
    setStage("done");
  };
  return (
    <div class="rigcheck">
      <h4 class="rigcheck__title">
        Practice rig check <span class="rigcheck__audible">Makes a sound</span>
      </h4>
      <p class="hint">
        With the mixer plugged in and your in-ear monitors on: measures each input channel, asks you to say a few words, plays three short test bursts through the
        current output and listens whether the mixer sends them back, and how late. The result goes into a report for the next session.
      </p>
      {stage === "idle" || stage === "done" ? (
        <button type="button" class="button button--small" onClick={() => setStage("confirm")}>
          Run the rig check…
        </button>
      ) : null}
      {stage === "confirm" && (
        <div class="rigcheck__confirm" role="alertdialog" aria-label="The rig check makes a sound">
          <p>
            <strong>This plays a test sound</strong> (three short bursts at -24 dBFS, through the current output at the app's volume). Put your in-ear monitors on first.
          </p>
          <label class="field">
            <span class="field__label">Note for this run (optional)</span>
            <input type="text" value={label} placeholder="for example: loopback on" onInput={(e) => setLabel((e.currentTarget as HTMLInputElement).value)} />
          </label>
          <div class="rigcheck__actions">
            <button type="button" class="button button--primary" onClick={() => void start()}>
              Start (makes a sound)
            </button>
            <button type="button" class="button" onClick={() => setStage("idle")}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {stage === "running" && <p role="status">{step?.message ?? "Starting…"}</p>}
      {error && <p class="note note--error">{error}</p>}
      {report && (
        <div class="rigcheck__report" role="status">
          <ul>
            {report.findings.map((f) => (
              <li key={f}>{f}</li>
            ))}
          </ul>
          <p class="hint">Report: {report.report_path}</p>
        </div>
      )}
    </div>
  );
}

/** Changes one jukebox customisation (ui.look; the core keeps it as it is). */
function setLook(changes: Partial<LookSettings>): void {
  changeSettings({ ui: { look: changes } });
}

/** Applies a theme: every look option and the lyric colours at once (again = reset to it). */
export function applyTheme(t: Theme): void {
  changeSettings({ ui: { look: { ...t.look, theme: t.id }, colors: { ...t.colors } } });
}

/**
 * Themes (owner, 2026-11-23): one click sets every part of the look; each part can then
 * be changed on its own below. Clicking the theme in use again resets it to the preset.
 */
function Themes() {
  const ui = useApp((s) => s.settings?.ui);
  if (!ui) return null;
  const { theme, modified } = themeState(ui);
  return (
    <Section title="Themes">
      <div class="themes" role="group" aria-label="Themes">
        {THEMES.map((t) => {
          const active = theme?.id === t.id;
          return (
            <button
              key={t.id}
              type="button"
              class={`theme-card${active ? " theme-card--active" : ""}`}
              aria-pressed={active}
              onClick={() => applyTheme(t)}
              title={active && modified ? `Reset ${t.name} (undo your changes to it)` : `Use the ${t.name} theme`}
            >
              <span class="theme-card__swatch" aria-hidden="true" style={`background:linear-gradient(135deg, ${t.swatch[0]} 0%, ${t.swatch[0]} 33%, ${t.swatch[1]} 33%, ${t.swatch[1]} 66%, ${t.swatch[2]} 66%)`} />
              <span class="theme-card__name">{t.name}</span>
              <span class="theme-card__blurb">{t.blurb}</span>
              {active && <span class="theme-card__state">{modified ? "In use, changed · click to reset" : "In use"}</span>}
            </button>
          );
        })}
      </div>
      <p class="hint">
        A theme sets the cabinet, screen border, metals, badge, glow, lane and lyric colours and effects together. Change any of them below afterwards; click the theme again to put it back as it was.
        {!ui.skin && " Turn on the jukebox look above to see the cabinet and border."}
      </p>
    </Section>
  );
}

/** The owner's jukebox customisation (2026-11-22): cabinet, screen border, chrome, badge, lattice, trim. */
function JukeboxCustomisation() {
  const ui = useApp((s) => s.settings?.ui);
  const platform = useApp((s) => s.info?.platform ?? "windows");
  if (!ui) return null;
  const look = lookOf(ui);
  const off = !ui.skin;
  const { theme } = themeState(ui);
  return (
    <Section title="Customize the jukebox">
      <Select<LookSettings["cabinet"]>
        label="Cabinet"
        value={look.cabinet}
        disabled={off}
        options={[
          { value: "pearl", label: "Pearlescent glow (slowly turning warm lights)" },
          { value: "walnut", label: "Dark walnut" },
          { value: "maple", label: "Light maple" },
          { value: "aquarium", label: "Aquarium (fish, plants and bubbles)" },
          { value: "forge", label: "Forge (flowing lava)" },
          { value: "ocean", label: "Ocean (rolling waves)" },
          { value: "brass", label: "Steampunk brass (riveted plates, turning gears, steam)" },
          { value: "cyber", label: "Cyberpunk city (neon towers, billboards, rain, wet street)" },
        ]}
        onChange={(v) => setLook({ cabinet: v })}
      />
      <Select<LookSettings["frame"]>
        label="Screen border"
        value={look.frame}
        disabled={off}
        options={[
          { value: "neon", label: "Neon bubble tubes" },
          { value: "pilaster", label: "Pearl pilasters with chrome rings and two small bubble tubes" },
          { value: "lava", label: "Lava lamp" },
          { value: "light", label: "Lightning tube (blue water, purple lightning)" },
          { value: "ocean", label: "Ocean tube (fish, kelp, coral; goes with the aquarium)" },
          { value: "fire", label: "Fire pipe (one long flame, no water)" },
          { value: "water", label: "Water pipe (blue water coursing through)" },
          { value: "steam", label: "Steam pipe (white steam)" },
          { value: "data", label: "Data pipe (neon, data racing both ways)" },
        ]}
        onChange={(v) => setLook({ frame: v })}
      />
      <Select<LookSettings["accents"]>
        label="Bands between the chrome rings"
        value={look.accents}
        disabled={off}
        options={[
          { value: "silver", label: "Silver chrome" },
          { value: "red", label: "Red enamel" },
          { value: "iron", label: "Iron (the inner trim turns iron too)" },
          { value: "copper", label: "Copper (the inner trim turns copper too)" },
        ]}
        onChange={(v) => setLook({ accents: v })}
      />
      <Select<LookSettings["metal"]>
        label="Metal"
        value={look.metal}
        disabled={off}
        options={[
          { value: "gold", label: "Gold chrome" },
          { value: "bronze", label: "Bronze" },
          { value: "chrome", label: "Chrome" },
          { value: "brass", label: "Brass" },
        ]}
        onChange={(v) => setLook({ metal: v })}
      />
      <Select<LookSettings["glow"]>
        label="Glow colour"
        value={look.glow}
        options={[
          { value: "pink", label: "Neon pink (red speaker cloth)" },
          { value: "ember", label: "Ember orange" },
          { value: "aqua", label: "Aqua" },
          { value: "violet", label: "Violet" },
          { value: "amber", label: "Amber" },
        ]}
        onChange={(v) => setLook({ glow: v })}
      />
      <Select<LookSettings["badge_field"]>
        label="Badge"
        value={look.badge_field}
        disabled={off}
        options={[
          { value: "lapis", label: "Lapis lazuli" },
          { value: "red", label: "Red enamel" },
          { value: "silver", label: "Silver chrome" },
          { value: "magma", label: "Magma (glowing cracks)" },
          { value: "copper", label: "Riveted copper" },
          { value: "carbon", label: "Carbon fibre" },
        ]}
        onChange={(v) => setLook({ badge_field: v })}
      />
      <Select<LookSettings["badge_rim"]>
        label="Badge frame"
        value={look.badge_rim}
        disabled={off}
        options={[
          { value: "lapis", label: "Lapis lazuli with a thin metal edge" },
          { value: "gold", label: "Chrome (in the metal chosen above)" },
        ]}
        onChange={(v) => setLook({ badge_rim: v })}
      />
      <Select<LookSettings["badge_text"]>
        label="Badge lettering"
        value={look.badge_text}
        disabled={off}
        options={[
          { value: "gold", label: "Chrome (in the metal chosen above)" },
          { value: "cream", label: "Cream" },
          { value: "ember", label: "Ember (glowing)" },
          { value: "neon", label: "Neon (glowing cyan and yellow)" },
          { value: "white", label: "Glossy white" },
        ]}
        onChange={(v) => setLook({ badge_text: v })}
      />
      <Select<LookSettings["lattice"]>
        label="Speaker lattice"
        value={look.lattice}
        disabled={off}
        options={[
          { value: "silver", label: "Silver (metal surround)" },
          { value: "gold", label: "Gold" },
          { value: "iron", label: "Iron" },
        ]}
        onChange={(v) => setLook({ lattice: v })}
      />
      <Toggle
        label="Inner trim"
        checked={look.inner_trim}
        disabled={off}
        onChange={(v) => setLook({ inner_trim: v })}
        hint="A thin silver chrome line (iron with iron bands) inside the gold or bronze of the buttons, the key dial, the speaker grilles and the medallions."
      />
      <Toggle
        label="GPU lava and fire (Windows)"
        checked={look.gpu_fx}
        disabled={off || platform !== "windows"}
        onChange={(v) => setLook({ gpu_fx: v })}
        hint={
          platform === "windows"
            ? "The Forge cabinet as flowing lava and the Fire pipe's flame, drawn by the graphics card. Off: lighter versions for older computers."
            : "Windows only. This Mac draws the lighter versions."
        }
      />
      <button type="button" class="button button--small" disabled={off} onClick={() => applyTheme(theme ?? THEMES[0])}>
        Reset to {(theme ?? THEMES[0]).name}
      </button>
      {off && <p class="hint">Turn on the jukebox look above to see these.</p>}
    </Section>
  );
}

/** Practice mode (WO2, APP_API 12.7): per computer, saved in settings.json on the stick. */
function PracticeSettings() {
  const settings = useApp((s) => s.settings);
  const [manual, setManual] = useState(0);
  if (!settings) return null;
  const pr = { ...DEFAULT_PRACTICE_SETTINGS, ...(settings.practice ?? {}) };
  return (
    <Section title="Practice">
      <Toggle
        label="Watch only follows the singer"
        checked={lookOf(settings.ui).watch_follow}
        onChange={(v) => setLook({ watch_follow: v })}
        hint="With no microphone (Watch only), a perfect voice rides the melody: the marker, its trail and the lit notes of a flawless take, so a performance without a microphone still shows a singer on the melody."
      />
      <Toggle
        label="Celebrate perfect phrases"
        checked={lookOf(settings.ui).celebrate}
        onChange={(v) => setLook({ celebrate: v })}
        hint="A burst of colour and a gold PERFECT! when a phrase is sung on pitch (and in Watch only when it follows the singer). Off when motion is reduced."
      />
      <Select<LookSettings["celebration"]>
        label="Celebration"
        value={lookOf(settings.ui).celebration}
        disabled={!lookOf(settings.ui).celebrate}
        options={[
          { value: "mix", label: "Mix (a different one each time)" },
          { value: "starburst", label: "Starburst" },
          { value: "fireworks", label: "Fireworks" },
          { value: "rings", label: "Neon rings" },
          { value: "bloom", label: "Colour bloom and confetti" },
          { value: "inferno", label: "Inferno (a blaze of flames)" },
        ]}
        onChange={(v) => setLook({ celebration: v })}
      />
      <Select<LookSettings["celebrate_freq"]>
        label="How often"
        value={lookOf(settings.ui).celebrate_freq}
        disabled={!lookOf(settings.ui).celebrate}
        options={[
          { value: "often", label: "More often (nearly perfect phrases count too)" },
          { value: "every", label: "Every perfect phrase" },
          { value: "second", label: "Every second perfect phrase" },
          { value: "third", label: "Every third perfect phrase" },
          { value: "rare", label: "Now and then (at most once every 30 seconds)" },
        ]}
        onChange={(v) => setLook({ celebrate_freq: v })}
      />
      <Select<LookSettings["pitch_fx"]>
        label="On-pitch effect"
        value={lookOf(settings.ui).pitch_fx}
        options={[
          { value: "sparks", label: "Sparks" },
          { value: "rings", label: "Halo rings" },
          { value: "stars", label: "Twinkling stars" },
          { value: "flames", label: "Flames" },
          { value: "notes", label: "Music notes" },
          { value: "off", label: "Off" },
        ]}
        onChange={(v) => setLook({ pitch_fx: v })}
      />
      <Select<LookSettings["lane_bars"]>
        label="Note bars"
        value={lookOf(settings.ui).lane_bars}
        options={[
          { value: "pink", label: "Neon pink" },
          { value: "ember", label: "Ember orange" },
          { value: "aqua", label: "Aqua" },
          { value: "violet", label: "Violet" },
          { value: "amber", label: "Amber" },
        ]}
        onChange={(v) => setLook({ lane_bars: v })}
      />
      <p class="hint">Comes off the pitch cursor while you are on pitch (and from the voice that follows the singer in Watch only). Off when motion is reduced.</p>
      <Select<"left" | "right">
        label="Piano keys"
        value={pr.piano_side}
        options={[
          { value: "left", label: "At the left edge of the notes" },
          { value: "right", label: "At the right edge of the notes" },
        ]}
        onChange={(v) => changeSettings({ practice: { piano_side: v } })}
      />
      <Toggle label="Words under the notes" checked={pr.words_under_notes} onChange={(v) => changeSettings({ practice: { words_under_notes: v } })} hint="Each word rides under its note in the lane." />
      <Toggle label="Lyrics under the lane" checked={pr.lyrics_below} onChange={(v) => changeSettings({ practice: { lyrics_below: v } })} hint="The usual karaoke lyrics, in the display chosen above." />
      <Toggle
        label="Simple lane"
        checked={pr.simple_lane}
        onChange={(v) => changeSettings({ practice: { simple_lane: v } })}
        hint="No glow and no tail behind the marker: lighter work for a slow computer. Reduce motion also drops the tail."
      />
      <Select<Strictness>
        label="How strict"
        value={pr.strictness}
        options={[
          { value: "relaxed", label: "Relaxed (full marks within 3/4 of a semitone)" },
          { value: "standard", label: "Standard (full marks within half a semitone)" },
          { value: "strict", label: "Strict (full marks within a third of a semitone)" },
        ]}
        onChange={(v) => changeSettings({ practice: { strictness: v } })}
      />
      <Select<OctaveMode>
        label="Octave for songs not practised yet"
        value={pr.octave_mode}
        options={[
          { value: "auto", label: "Automatic: follow the octave I sing in" },
          { value: "fixed", label: "As written (change it per song in practice)" },
          { value: "strict", label: "Strict: the written octave is required" },
        ]}
        onChange={(v) => changeSettings({ practice: { octave_mode: v } })}
      />
      <p class="hint">Singing the right note an octave higher or lower counts as right, except with Strict. Each song remembers its own octave and strictness.</p>
      <Toggle
        label="Measure the delay automatically"
        checked={pr.auto_latency}
        onChange={(v) => changeSettings({ practice: { auto_latency: v } })}
        hint="Uses the delay measured when the mixer sends the app's music back, or by the practice rig check."
      />
      <Slider label="Manual delay for this microphone" value={manual} min={-300} max={300} step={5} format={formatOffset} onInput={(v) => setManual(v)} />
      <button type="button" class="button button--small" onClick={() => void setLatency(manual)}>
        Use this delay
      </button>
      <p class="hint">Only if your voice seems late or early against the notes and nothing could be measured. Small delays are forgiven anyway and never cost points.</p>
      <RigCheck />
    </Section>
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
  /** The page display (static lines): no fixed marker; its progress bar rides the wipe edge (settings.ui.show_marker). */
  const page = (ui.lyrics_display ?? "page") === "page";
  const showMarker = ui.show_marker ?? true;

  return (
    <div class="settings">
      <Section title="Look">
        <Toggle label="Jukebox look" checked={ui.skin} onChange={(v) => changeSettings({ ui: { skin: v } })} hint="Off: a plain, flat layout." />
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
        <Select<LookSettings["fx_fps"]>
          label="Frame rate (Windows)"
          value={lookOf(ui).fx_fps}
          disabled={info.platform !== "windows"}
          options={[
            { value: "standard", label: "Standard (lightest: effects 8-30 fps, lyrics and notes 60 fps)" },
            { value: "60", label: "Up to 60 fps" },
            { value: "120", label: "Up to 120 fps" },
            { value: "240", label: "Up to 240 fps (as fast as the screen refreshes)" },
          ]}
          onChange={(v) => setLook({ fx_fps: v })}
        />
        <p class="hint">{info.platform === "windows" ? "Bubbles, lava, fire, lightning, fish, the drifting light, celebrations, the notes lane and the lyrics. Higher rates look smoother on a fast screen and use more power." : "Windows only. This Mac keeps the standard rates."}</p>
      </Section>

      <Themes />

      <JukeboxCustomisation />

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
        <p class="hint">Static lines: each line stands still and the sung colour sweeps through it word by word, with a slim progress bar riding the edge of the sweep; a small block before a line runs out when it is time to sing. Scrolling ticker: one line of text slides under a fixed marker.</p>
        <Toggle
          label="Show the next line"
          checked={ui.show_next_line ?? true}
          disabled={!page}
          onChange={(v) => changeSettings({ ui: { show_next_line: v } })}
          hint={page ? "Smaller, under the line being sung, so you can read ahead." : "Static lines only (the ticker shows what is coming by itself)."}
        />
        <Toggle
          label="Show the progress bar"
          checked={showMarker}
          disabled={!page}
          onChange={(v) => changeSettings({ ui: { show_marker: v } })}
          hint={page ? "A slim bar in the marker colour that moves with the sung colour, exactly where the song is." : "Static lines only (the ticker always has its fixed marker)."}
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
        <p class="hint">The size on a 1440 × 900 window; it scales with the window. Static lines: a line too long for the screen is made smaller or split over two rows. Ticker: if a fast passage would not leave at least 2 seconds of upcoming words on screen, the text is made smaller for that song.</p>
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
          <ColorField label="Marker" value={ui.colors.marker} disabled={page && !showMarker} onChange={(v) => changeSettings({ ui: { colors: { marker: v } } }, 300)} />
          <button type="button" class="button button--small" onClick={() => changeSettings({ ui: { colors: { ...DEFAULT_APP_SETTINGS.ui.colors } } })}>
            Default colours
          </button>
        </div>
      </Section>

      <PracticeSettings />

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
        </dl>
        {/* The web engine's user-agent string is for bug reports, not for reading: folded away. */}
        <details class="settings__details">
          <summary>Details</summary>
          <dl class="facts">
            <dt>Web engine</dt>
            <dd>
              <code>{navigator.userAgent}</code>
            </dd>
          </dl>
        </details>
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
