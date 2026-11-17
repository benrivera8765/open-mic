// SPDX-License-Identifier: GPL-3.0-or-later
// skin-demo.html: the whole frame with placeholder content and every skin
// component, so the skin can be judged in a browser alone (`npm run dev`, then
// open /skin-demo.html). Not part of the app bundle.
//
// URL parameters (used by tools/measure-skin.mjs for screenshots and timing):
//   view=library|stage|gallery   bubbles=0|1   motion=full|reduce|system
//   paused=1   hud=0 (hide the demo controls)
//
// Song titles are public-domain traditional songs; no lyrics beyond the opening
// line of "Twinkle, Twinkle, Little Star" (Jane Taylor, 1806).

import { render } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import {
  Badge,
  bubbleStats,
  CrestOrnament,
  Fader,
  formatSemitones,
  Grille,
  JukeboxFrame,
  KeyDial,
  Medallion,
  PlateButton,
  TitleStripCard,
  TitleStripRack,
  TransportIcon,
  useResolvedMotion,
  type MotionSetting,
} from "../index";
import "./demo.css";

type View = "library" | "stage" | "gallery";

const params = new URLSearchParams(location.search);
const initialView = (["library", "stage", "gallery"].includes(params.get("view") ?? "") ? params.get("view") : "library") as View;
const initialMotion = (["full", "reduce", "system"].includes(params.get("motion") ?? "") ? params.get("motion") : "system") as MotionSetting;

// Expose live counters to the measurement script.
(window as unknown as { __jkSkin: unknown }).__jkSkin = { bubbleStats };

const NOTE_NAMES = ["C", "D♭", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

/** Demo-only key naming: "A major -> B major (+2)" (the app gets this from Rust). */
function keyLabel(tonic: number, mode: "major" | "minor", semitones: number): string {
  const from = `${NOTE_NAMES[tonic]} ${mode}`;
  const to = `${NOTE_NAMES[(((tonic + semitones) % 12) + 12) % 12]} ${mode}`;
  return semitones === 0 ? `${from} (original key)` : `${from} -> ${to} (${formatSemitones(semitones)})`;
}

interface Song {
  code: string;
  title: string;
  artist: string;
  tonic: number;
  mode: "major" | "minor";
  duration: number;
  note?: string;
  cover?: boolean;
}

const SONGS: Song[] = [
  { code: "A1", title: "Twinkle, Twinkle, Little Star", artist: "Jane Taylor (1806)", tonic: 2, mode: "major", duration: 134, cover: true },
  { code: "A2", title: "Amazing Grace", artist: "John Newton (1779)", tonic: 7, mode: "major", duration: 228 },
  { code: "A3", title: "Oh! Susanna", artist: "Stephen Foster (1848)", tonic: 5, mode: "major", duration: 151 },
  { code: "A4", title: "Scarborough Fair", artist: "Traditional", tonic: 2, mode: "minor", duration: 192, cover: true },
  { code: "A5", title: "Greensleeves", artist: "Traditional", tonic: 9, mode: "minor", duration: 185 },
  { code: "A6", title: "Shenandoah", artist: "Traditional", tonic: 2, mode: "major", duration: 220, note: "Lyrics transcribed - unchecked" },
  { code: "A7", title: "Home on the Range", artist: "Brewster M. Higley (1872)", tonic: 7, mode: "major", duration: 178 },
  { code: "A8", title: "Danny Boy", artist: "Frederic Weatherly (1913)", tonic: 3, mode: "major", duration: 210 },
  { code: "B1", title: "Auld Lang Syne", artist: "Robert Burns (1788)", tonic: 5, mode: "major", duration: 165 },
  { code: "B2", title: "Oh My Darling, Clementine", artist: "Traditional (1884)", tonic: 0, mode: "major", duration: 140 },
  { code: "B3", title: "Camptown Races", artist: "Stephen Foster (1850)", tonic: 10, mode: "major", duration: 125 },
  { code: "B4", title: "She'll Be Coming 'Round the Mountain When She Comes", artist: "Traditional", tonic: 0, mode: "major", duration: 150 },
  { code: "B5", title: "The Water Is Wide", artist: "Traditional", tonic: 4, mode: "major", duration: 205 },
  { code: "B6", title: "Swing Low, Sweet Chariot", artist: "Traditional", tonic: 8, mode: "major", duration: 198 },
];

const SHORT = (s: Song) => `${NOTE_NAMES[s.tonic]} ${s.mode === "major" ? "maj" : "min"}`;

/** A generated cover (no images from anywhere): a sunburst in the palette. */
function makeCover(hueA: string, hueB: string): string {
  const c = document.createElement("canvas");
  c.width = c.height = 96;
  const ctx = c.getContext("2d");
  if (!ctx) return "";
  const g = ctx.createLinearGradient(0, 0, 96, 96);
  g.addColorStop(0, hueA);
  g.addColorStop(1, hueB);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 96, 96);
  ctx.translate(48, 60);
  for (let i = 0; i < 16; i++) {
    ctx.rotate(Math.PI / 8);
    ctx.fillStyle = i % 2 ? "rgba(255,255,255,0.18)" : "rgba(0,0,0,0.08)";
    ctx.beginPath();
    ctx.moveTo(0, 0);
    ctx.lineTo(80, -10);
    ctx.lineTo(80, 10);
    ctx.fill();
  }
  ctx.fillStyle = "#f6efd9";
  ctx.beginPath();
  ctx.arc(0, 0, 13, 0, Math.PI * 2);
  ctx.fill();
  return c.toDataURL("image/png");
}

function Library({ selected, onSelect }: { selected: string; onSelect(code: string): void }) {
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<"title" | "artist" | "key">("title");
  const covers = useMemo(() => [makeCover("#ff3d8b", "#ff8a1f"), makeCover("#4dff6a", "#9b6bff")], []);
  const songs = SONGS.filter((s) => `${s.title} ${s.artist}`.toLowerCase().includes(query.toLowerCase())).sort((a, b) =>
    sort === "title" ? a.title.localeCompare(b.title) : sort === "artist" ? a.artist.localeCompare(b.artist) : a.tonic - b.tonic,
  );
  return (
    <div class="demo-library">
      <div class="demo-topbar">
        <input
          class="jk-glass-input demo-search"
          type="search"
          placeholder="Search songs or artists"
          value={query}
          onInput={(e) => setQuery((e.currentTarget as HTMLInputElement).value)}
        />
        <div class="demo-sort">
          <PlateButton size="sm" label="Title" active={sort === "title"} onClick={() => setSort("title")} />
          <PlateButton size="sm" label="Artist" active={sort === "artist"} onClick={() => setSort("artist")} />
          <PlateButton size="sm" label="Key" active={sort === "key"} onClick={() => setSort("key")} />
        </div>
        <span class="demo-free">21.4 GB free on the stick</span>
      </div>
      <div class="demo-scroll">
        <TitleStripRack label="Library">
          {songs.map((s, i) => (
            <TitleStripCard
              key={s.code}
              code={s.code}
              title={s.title}
              artist={s.artist}
              keyName={SHORT(s)}
              duration={s.duration}
              cover={s.cover ? covers[i % 2] : null}
              note={s.note}
              selected={selected === s.code}
              onSelect={() => onSelect(s.code)}
            />
          ))}
        </TitleStripRack>
      </div>
    </div>
  );
}

function Stage({ song, label }: { song: Song; label: string }) {
  return (
    <div class="demo-stage">
      <div class="demo-stage-info">
        <span class="demo-stage-title">{song.title}</span>
        <span>{song.artist}</span>
        <span>{label.replace(/\s*->\s*/g, " → ")}</span>
        <span class="demo-stage-time">0:42 / 2:14</span>
      </div>
      <div class="demo-ticker jk-lyrics-text" aria-label="Lyrics ticker placeholder">
        <div class="demo-ticker-sung">Twinkle, twinkle, little st</div>
        <div class="demo-ticker-up">ar, how I wonder what you are</div>
        <div class="demo-marker" />
      </div>
      <div class="demo-stage-note">Placeholder: the real ticker is a canvas from the UI stream.</div>
    </div>
  );
}

function Gallery({ semitones, setSemitones }: { semitones: number; setSemitones(n: number): void }) {
  const [a, setA] = useState(0.35);
  const [b, setB] = useState(0.7);
  return (
    <div class="demo-gallery">
      <section>
        <h2>Plate buttons</h2>
        <div class="demo-row">
          <PlateButton label="Make selections" />
          <PlateButton label="Play" active />
          <PlateButton label="Record" disabled />
          <PlateButton size="sm" label={"−"} ariaLabel="Minus" />
          <PlateButton size="lg" label="Library" />
        </div>
      </section>
      <section>
        <h2>Key dial</h2>
        <div class="demo-row">
          <KeyDial semitones={semitones} label={keyLabel(9, "major", semitones)} onChange={setSemitones} />
          <KeyDial semitones={-5} label="E minor -> B minor (-5)" onChange={() => undefined} disabled />
        </div>
      </section>
      <section>
        <h2>Faders</h2>
        <div class="demo-row">
          <Fader label="Guide" value={a} onChange={setA} />
          <Fader label="Backing" value={b} onChange={setB} />
          <Fader label="Level" value={b} onChange={setB} orientation="horizontal" />
          <Fader label="Off" value={0.5} onChange={() => undefined} disabled />
        </div>
      </section>
      <section>
        <h2>Ornaments</h2>
        <div class="demo-row demo-ornaments">
          <Badge class="demo-badge-large" />
          <Medallion class="demo-medallion" title="Compass star medallion" />
          <div class="demo-grille-panel">
            <Grille />
          </div>
          <CrestOrnament class="demo-crest" />
        </div>
      </section>
      <section>
        <h2>Title strips</h2>
        <div class="demo-strips">
          <TitleStripCard code="C3" title="Greensleeves" artist="Traditional" keyName="A min" duration={185} onSelect={() => undefined} />
          <TitleStripCard code="C4" title="Amazing Grace" artist="John Newton (1779)" keyName="G maj" duration={228} selected onSelect={() => undefined} />
          <TitleStripCard code="C5" title="Shenandoah" artist="Traditional" keyName={null} duration={220} note="Lyrics transcribed - unchecked" onSelect={() => undefined} />
        </div>
      </section>
    </div>
  );
}

function Demo() {
  const [view, setView] = useState<View>(initialView);
  const [bubbles, setBubbles] = useState(params.get("bubbles") !== "0");
  const [motionSetting, setMotionSetting] = useState<MotionSetting>(initialMotion);
  const [paused, setPaused] = useState(params.get("paused") === "1");
  const [hud, setHud] = useState(params.get("hud") !== "0");
  const [semitones, setSemitones] = useState(2);
  const [guide, setGuide] = useState(0);
  const [backing, setBacking] = useState(0.8);
  const [master, setMaster] = useState(0.9);
  const [playing, setPlaying] = useState(false);
  const [selected, setSelected] = useState("A1");
  const motion = useResolvedMotion(motionSetting);
  const song = SONGS.find((s) => s.code === selected) ?? SONGS[0];
  const label = keyLabel(song.tonic, song.mode, semitones);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement) return;
      if (e.key === "d" || e.key === "D") setHud((h) => !h);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const consoleContent = (
    <div class="demo-console">
      <div class="demo-console-group">
        <PlateButton label={view === "stage" ? "Library" : "Stage"} onClick={() => setView(view === "stage" ? "library" : "stage")} title="Switch view" />
        <div class="demo-transport">
          <PlateButton label={<TransportIcon name="restart" />} ariaLabel="Restart" title="Restart" />
          <PlateButton
            label={<TransportIcon name={playing ? "pause" : "play"} />}
            ariaLabel={playing ? "Pause" : "Play"}
            active={playing}
            onClick={() => setPlaying(!playing)}
          />
          <PlateButton label={<TransportIcon name="stop" />} ariaLabel="Stop" title="Stop" onClick={() => setPlaying(false)} />
        </div>
      </div>
      <KeyDial semitones={semitones} label={label} onChange={setSemitones} />
      <div class="demo-console-group demo-mixer">
        <Fader label="Guide" value={guide} onChange={setGuide} />
        <Fader label="Backing" value={backing} onChange={setBacking} />
        <Fader label="Master" value={master} onChange={setMaster} />
      </div>
    </div>
  );

  return (
    <>
      <JukeboxFrame console={consoleContent} crest={<Badge />} bubbles={bubbles} motion={motion} paused={paused}>
        {view === "library" && <Library selected={selected} onSelect={setSelected} />}
        {view === "stage" && <Stage song={song} label={label} />}
        {view === "gallery" && <Gallery semitones={semitones} setSemitones={setSemitones} />}
      </JukeboxFrame>
      {hud && (
        <div class="demo-hud" role="group" aria-label="Demo controls (press D to hide)">
          <span class="demo-hud-title">Skin demo</span>
          <select value={view} onChange={(e) => setView((e.currentTarget as HTMLSelectElement).value as View)}>
            <option value="library">Library</option>
            <option value="stage">Stage</option>
            <option value="gallery">Components</option>
          </select>
          <label>
            <input type="checkbox" checked={bubbles} onChange={(e) => setBubbles((e.currentTarget as HTMLInputElement).checked)} /> Bubbles
          </label>
          <select value={motionSetting} onChange={(e) => setMotionSetting((e.currentTarget as HTMLSelectElement).value as MotionSetting)}>
            <option value="system">Motion: system</option>
            <option value="full">Motion: full</option>
            <option value="reduce">Motion: reduce</option>
          </select>
          <label>
            <input type="checkbox" checked={paused} onChange={(e) => setPaused((e.currentTarget as HTMLInputElement).checked)} /> Paused
          </label>
        </div>
      )}
    </>
  );
}

const root = document.getElementById("app");
if (root) render(<Demo />, root);
