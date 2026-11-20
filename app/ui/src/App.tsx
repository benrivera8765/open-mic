// SPDX-License-Identifier: GPL-3.0-or-later
// The app: navigation, the current screen, and the console (transport + key control).
// Two looks over the same screens and actions (state/app.ts):
//
//   - settings.ui.skin on (the default): the jukebox (src/skin). The window is a
//     JukeboxFrame; the screens sit in its screen with plate-button navigation on
//     top; the console holds the transport plates, the KeyDial and the faders
//     (components/Jukebox.tsx); the library is TitleStripCards. settings.ui.bubbles
//     and settings.ui.motion ("system" follows prefers-reduced-motion) drive the
//     frame's bubbles and light drift. skin-app.css restyles the plain components
//     (panels, buttons, fields) for the dark screen.
//   - skin off: the plain functional layout (header, screen, console). Tests and
//     scripts run against both.
//
// Full-screen mode shows the stage alone (inside the frame when the skin is on).
// Keyboard shortcuts are handled here, app-wide (see handleShortcut), in both looks.
//
// The data-* attributes set on <html>: data-skin, data-bubbles, data-motion
// (resolved: "reduce" or "full"), data-platform, data-fullscreen, data-screen.
// Colours chosen in Settings are exposed as CSS variables --sung, --upcoming and
// --marker-color.
import { useEffect, useState } from "preact/hooks";
import type { Api } from "./api/client";
import type { AppInfo } from "./api/types";
import { SkinConsole, useWindowMinimised } from "./components/Jukebox";
import { KeyControl } from "./components/KeyControl";
import { Library } from "./components/Library";
import { Mixer } from "./components/Mixer";
import { HelpOverlay, SelfCheckOverlay } from "./components/Overlays";
import { Recorder } from "./components/Recorder";
import { Settings } from "./components/Settings";
import { Stage } from "./components/Stage";
import { Studio } from "./components/Studio";
import { SyncEditor } from "./components/SyncEditor";
import { Transport } from "./components/Transport";
import { dismissToast, focusSearch, go, init, loadSong, play, type Screen, seek, seekBy, setFullscreen, setOverlay, shiftKey, store, togglePlay, useApp } from "./state/app";
import { reportSmoke } from "./lib/smoke";
import { errorMessage } from "./api/client";
import { Badge, bubbleStats, JukeboxFrame, PlateButton, useResolvedMotion } from "./skin";
import { openPicker, startPractice, togglePractice, watchOnly as practiceWatchOnly } from "./state/practice";

const NAV: { screen: Screen; label: string; needsSong?: boolean }[] = [
  { screen: "library", label: "Library" },
  { screen: "stage", label: "Sing" },
  { screen: "sync", label: "Fix timing", needsSong: true },
  { screen: "recorder", label: "Recorder" },
  { screen: "studio", label: "Studio" },
  { screen: "settings", label: "Settings" },
];

/** True when the key event is typing into a text field (shortcuts must not fire). */
function isTyping(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable) return true;
  if (t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement) return true;
  if (t instanceof HTMLInputElement) return !["checkbox", "radio", "button", "submit", "reset", "color", "range"].includes(t.type);
  return false;
}

/** The app-wide keyboard shortcuts (documented in the "?" overlay). */
export function handleShortcut(e: KeyboardEvent): void {
  const s = store.get();
  if (s.tapMode || e.defaultPrevented) return;
  const t = e.target;
  const mod = e.ctrlKey || e.metaKey;
  if (mod && !e.altKey && (e.key === "f" || e.key === "F")) {
    e.preventDefault();
    focusSearch();
    return;
  }
  if (e.key === "Escape") {
    if (s.overlay || s.modal) return; // the dialog closes itself
    if (s.fullscreen) {
      e.preventDefault();
      void setFullscreen(false);
    } else if (isTyping(t)) (t as HTMLElement).blur();
    return;
  }
  if (mod || e.altKey || s.overlay || s.modal || isTyping(t)) return;
  const onRange = t instanceof HTMLInputElement && t.type === "range";
  const onButton = t instanceof HTMLButtonElement || (t instanceof HTMLInputElement && ["checkbox", "radio"].includes(t.type));
  switch (e.key) {
    case " ":
    case "Spacebar":
      if (onButton) return; // keyboard users activate the focused button
      e.preventDefault();
      void togglePlay();
      return;
    case "ArrowLeft":
    case "ArrowRight":
      if (onRange) return;
      e.preventDefault();
      void seekBy(e.key === "ArrowLeft" ? -5 : 5);
      return;
    case "[":
    case "-":
    case "_":
      e.preventDefault();
      shiftKey(-1);
      return;
    case "]":
    case "+":
    case "=":
      e.preventDefault();
      shiftKey(1);
      return;
    case "f":
    case "F":
      if (!s.song) return;
      e.preventDefault();
      void setFullscreen(!s.fullscreen);
      return;
    case "?":
      e.preventDefault();
      setOverlay("help");
      return;
    case "p":
    case "P":
      // WO2 4.5: practice mode on the singing screen.
      if (!s.song) return;
      e.preventDefault();
      if (s.screen !== "stage" && !s.fullscreen) go("stage");
      togglePractice();
      return;
  }
}

function useDocumentAttributes(): void {
  const settings = useApp((s) => s.settings);
  const platform = useApp((s) => s.info?.platform ?? "windows");
  const fullscreen = useApp((s) => s.fullscreen);
  const screen = useApp((s) => s.screen);
  const [systemReduce, setSystemReduce] = useState(() => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!mq) return;
    const on = () => setSystemReduce(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  useEffect(() => {
    const root = document.documentElement;
    const ui = settings?.ui;
    const reduce = ui ? ui.motion === "reduce" || (ui.motion === "system" && systemReduce) : systemReduce;
    root.dataset.skin = ui?.skin === false ? "off" : "on";
    root.dataset.bubbles = ui?.bubbles && !reduce ? "on" : "off";
    root.dataset.motion = reduce ? "reduce" : "full";
    root.dataset.platform = platform;
    root.dataset.fullscreen = fullscreen ? "on" : "off";
    root.dataset.screen = screen;
    if (ui) {
      root.style.setProperty("--sung", ui.colors.sung);
      root.style.setProperty("--upcoming", ui.colors.upcoming);
      root.style.setProperty("--marker-color", ui.colors.marker);
    }
  }, [settings, platform, fullscreen, screen, systemReduce]);
}

/** Browser mock only: URL switches that open a state directly (screenshots, manual checks). */
async function mockStartup(search: string): Promise<void> {
  const q = new URLSearchParams(search);
  const songs = (store.get().library?.songs ?? []).filter((s) => !s.error);
  if (q.has("song")) {
    const song = songs[Number(q.get("song")) || 0];
    if (song && (await loadSong(song.song_id))) {
      const at = Number(q.get("at") ?? 0);
      if (at > 0) await seek(at);
      if (q.get("play") === "1") await play();
    }
  }
  const screen = q.get("screen");
  if (screen && ["library", "stage", "sync", "recorder", "studio", "settings"].includes(screen)) go(screen as Screen);
  if (q.get("fullscreen") === "1") store.set({ fullscreen: true });
  const overlay = q.get("overlay");
  if (overlay === "help" || overlay === "selfcheck") setOverlay(overlay);
  // WO2: practice mode (screenshots, manual checks).
  const pr = q.get("practice");
  if (store.get().song && (pr === "1" || pr === "watch" || pr === "result")) {
    go("stage");
    if (pr === "watch") await practiceWatchOnly();
    else await startPractice();
    if (q.get("picker") === "1") await openPicker();
  }
}

/** macOS App Translocation (WORK_ORDER trap 2): explain, and offer to pick the stick's folder. */
function TranslocationBanner({ api, help }: { api: Api; help: string | null }) {
  const [msg, setMsg] = useState<string | null>(null);
  return (
    <div class="banner banner--warn" role="alert">
      <p>
        {help ??
          "macOS is running this app from a temporary copy, so it cannot see the folder it came from. Choose the stick's main folder (the one with Library and Mac in it), or follow the first-launch steps in the manual."}
      </p>
      {api.rootChoose && (
        <button
          type="button"
          class="button button--small"
          onClick={() =>
            void api.rootChoose!()
              .then((r) => setMsg(r.message))
              .catch((e) => setMsg(errorMessage(e)))
          }
        >
          Choose the stick's folder…
        </button>
      )}
      {msg && <p>{msg}</p>}
    </div>
  );
}

function Toasts() {
  const toasts = useApp((s) => s.toasts);
  return (
    <div class="toasts" aria-live="polite" aria-relevant="additions">
      {toasts.map((t) => (
        <div key={t.id} class={`toast toast--${t.kind}`} role={t.kind === "error" ? "alert" : "status"}>
          <span>{t.text}</span>
          <button type="button" class="icon-button toast__close" aria-label="Dismiss" onClick={() => dismissToast(t.id)}>
            <span aria-hidden="true">✕</span>
          </button>
        </div>
      ))}
    </div>
  );
}

function CurrentScreen({ screen, skin = false }: { screen: Screen; skin?: boolean }) {
  const [mixerOpen, setMixerOpen] = useState(false);
  switch (screen) {
    case "library":
      return <Library skin={skin} />;
    case "stage":
      return (
        <div class="stage-screen">
          <Stage />
          <div class="stage-screen__tools">
            <button type="button" class="button" aria-expanded={mixerOpen} onClick={() => setMixerOpen((v) => !v)}>
              {mixerOpen ? "Hide mixer" : "Mixer"}
            </button>
          </div>
          {mixerOpen && <Mixer />}
        </div>
      );
    case "sync":
      return <SyncEditor />;
    case "recorder":
      return <Recorder />;
    case "studio":
      return <Studio />;
    case "settings":
      return (
        <div class="settings-screen">
          <Settings />
          <Mixer />
        </div>
      );
  }
}

/** The bundled lyrics face (skin on). The ticker measures glyphs with it, so load it early. */
const SKIN_LYRICS_FONT = '700 64px "Atkinson Hyperlegible Next"';

/** Navigation as plate buttons along the top of the jukebox screen. */
function SkinNav() {
  const screen = useApp((s) => s.screen);
  const hasLyricsSong = useApp((s) => !!s.song?.lyrics);
  return (
    <header class="skin-nav">
      <nav class="skin-nav__screens" aria-label="Screens">
        {NAV.filter((n) => !n.needsSong || hasLyricsSong).map((n) => (
          <PlateButton
            key={n.screen}
            size="sm"
            class="skin-nav__item"
            label={n.label}
            active={screen === n.screen}
            onClick={() => go(n.screen)}
          />
        ))}
      </nav>
      <div class="skin-nav__tools">
        <PlateButton size="sm" label="Self-check" onClick={() => setOverlay("selfcheck")} />
        <PlateButton size="sm" label="?" ariaLabel="Keyboard shortcuts" title="Keyboard shortcuts (?)" onClick={() => setOverlay("help")} />
      </div>
    </header>
  );
}

/** The app inside the jukebox frame (settings.ui.skin on). */
function SkinApp({ api, info }: { api: Api; info: AppInfo }) {
  const ui = useApp((s) => s.settings?.ui);
  const screen = useApp((s) => s.screen);
  const fullscreen = useApp((s) => s.fullscreen);
  const overlay = useApp((s) => s.overlay);
  const motion = useResolvedMotion(ui?.motion ?? "system");
  const minimised = useWindowMinimised(api);
  useEffect(() => {
    // Ask for the lyrics face now; the ticker re-measures when fonts finish loading.
    document.fonts?.load(SKIN_LYRICS_FONT).catch(() => undefined);
    // Read-only diagnostics for scripts/real-app-screens.mjs (bubble frame counters).
    (window as unknown as { __jkSkin?: unknown }).__jkSkin = { bubbleStats };
  }, []);
  return (
    <>
      <a class="skip-link" href="#main">
        Skip to content
      </a>
      <JukeboxFrame
        class={`app-skin${fullscreen ? " app-skin--fullscreen" : ""}`}
        crest={<Badge name={info.name} />}
        bubbles={ui?.bubbles ?? true}
        motion={motion}
        paused={minimised}
        console={<SkinConsole />}
      >
        <div class="skin-screen">
          {/* The page heading (the badge in the crest is a picture of the same name);
              the --smoke-test also reads the rendered app name from the first h1. */}
          <h1 class="visually-hidden">{info.name}</h1>
          {!fullscreen && <SkinNav />}
          {info.translocated && !fullscreen && <TranslocationBanner api={api} help={(info as { translocation_help?: string | null }).translocation_help ?? null} />}
          <main class="app-main skin-main" id="main" tabIndex={-1}>
            {fullscreen ? <Stage /> : <CurrentScreen screen={screen} skin />}
          </main>
        </div>
      </JukeboxFrame>
      <Toasts />
      {overlay === "help" && <HelpOverlay />}
      {overlay === "selfcheck" && <SelfCheckOverlay />}
    </>
  );
}

export function App({ api, scriptStartMs }: { api: Api; scriptStartMs: number }) {
  const ready = useApp((s) => s.ready);
  const fatal = useApp((s) => s.fatal);
  const info = useApp((s) => s.info);
  const screen = useApp((s) => s.screen);
  const fullscreen = useApp((s) => s.fullscreen);
  const overlay = useApp((s) => s.overlay);
  const hasLyricsSong = useApp((s) => !!s.song?.lyrics);
  const hasSong = useApp((s) => !!s.song);
  const skin = useApp((s) => s.settings?.ui.skin !== false);
  useDocumentAttributes();

  useEffect(() => {
    const started = performance.now();
    void init(api).then(() => {
      if (api.kind === "mock") void mockStartup(window.location.search);
      if (api.sendSmokeReport) reportSmoke((r) => api.sendSmokeReport!(r), scriptStartMs, performance.now() - started);
    });
    window.addEventListener("keydown", handleShortcut);
    // Mouse clicks must not leave focus on buttons (Space would click them again
    // instead of play/pause); keyboard focus is unaffected.
    const noMouseFocus = (e: MouseEvent) => {
      if (e.target instanceof Element && e.target.closest("button")) e.preventDefault();
    };
    document.addEventListener("mousedown", noMouseFocus);
    const onFs = () => {
      if (!document.fullscreenElement && store.get().fullscreen && api.kind === "mock") store.set({ fullscreen: false });
    };
    document.addEventListener("fullscreenchange", onFs);
    return () => {
      window.removeEventListener("keydown", handleShortcut);
      document.removeEventListener("mousedown", noMouseFocus);
      document.removeEventListener("fullscreenchange", onFs);
    };
  }, []);

  if (fatal) {
    return (
      <main class="app app--fatal">
        <h1>The app could not start</h1>
        <p class="note note--error">{fatal}</p>
      </main>
    );
  }
  if (!ready || !info) {
    return (
      <main class="app app--loading" aria-busy="true">
        <h1>Starting…</h1>
      </main>
    );
  }

  if (skin) return <SkinApp api={api} info={info} />;

  return (
    <div class={`app${fullscreen ? " app--fullscreen" : ""}`}>
      <a class="skip-link" href="#main">
        Skip to content
      </a>
      {!fullscreen && (
        <header class="app-header">
          <h1 class="app-badge">{info.name}</h1>
          <nav class="app-nav" aria-label="Screens">
            {NAV.filter((n) => !n.needsSong || hasLyricsSong).map((n) => (
              <button key={n.screen} type="button" class={`app-nav__item${screen === n.screen ? " app-nav__item--current" : ""}`} aria-current={screen === n.screen ? "page" : undefined} onClick={() => go(n.screen)}>
                {n.label}
              </button>
            ))}
          </nav>
          <div class="app-header__tools">
            <button type="button" class="button button--small" onClick={() => setOverlay("selfcheck")}>
              Self-check
            </button>
            <button type="button" class="icon-button" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" onClick={() => setOverlay("help")}>
              <span aria-hidden="true">?</span>
            </button>
          </div>
        </header>
      )}
      {info.translocated && !fullscreen && <TranslocationBanner api={api} help={(info as { translocation_help?: string | null }).translocation_help ?? null} />}
      <main class="app-main" id="main" tabIndex={-1}>
        {fullscreen ? <Stage /> : <CurrentScreen screen={screen} />}
      </main>
      <footer class={`console${fullscreen ? " console--fullscreen" : ""}`} aria-label="Player controls">
        <Transport />
        <KeyControl compact={fullscreen} />
        {fullscreen && (
          <button type="button" class="button button--small" onClick={() => void setFullscreen(false)}>
            Leave full screen (Esc)
          </button>
        )}
        {!fullscreen && hasSong && screen !== "stage" && screen !== "sync" && (
          <button type="button" class="button button--small console__to-stage" onClick={() => go("stage")}>
            Back to the stage
          </button>
        )}
      </footer>
      <Toasts />
      {overlay === "help" && <HelpOverlay />}
      {overlay === "selfcheck" && <SelfCheckOverlay />}
    </div>
  );
}
