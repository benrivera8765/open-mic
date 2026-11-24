// SPDX-License-Identifier: GPL-3.0-or-later
// Stage: the singing view. Title, artist, key and time above the lyrics (the page display
// with its colour wipe, or the scrolling ticker: settings.ui.lyrics_display, D-218);
// full-screen mode shows only this. Class hooks: .stage, .stage--fullscreen,
// .stage__info, .stage__title, .stage__artist, .stage__key, .stage__time,
// .stage__lane, .stage__empty, .stage__unchecked, .stage__tools, .stage__practice,
// .stage__welcome (the no-song message), .stage__meta-form (Edit details).
//
// Practice mode (WO2 4.5) is a mode of this screen, not a second player: the Practice
// plate (or P) turns it on; the lane and the lyrics then share the lyrics area
// (practice/PracticeView.tsx) and the transport, key dial and mixer keep working. With
// practice off this screen is exactly as before.
import { useState } from "preact/hooks";
import { formatTime } from "../lib/format";
import { keyLabel } from "../lib/keys";
import { deviceOffsetMs, go, saveMetadata, setFullscreen, useApp } from "../state/app";
import { PageLyrics } from "../ticker/PageLyrics";
import { Ticker } from "../ticker/Ticker";
import { usePosition } from "./Transport";
import { PracticeTools, PracticeView } from "../practice/PracticeView";
import { togglePractice, usePractice } from "../state/practice";

function MetadataEditor({ onClose }: { onClose: () => void }) {
  const song = useApp((s) => s.song);
  const [title, setTitle] = useState(song?.summary.title ?? "");
  const [artist, setArtist] = useState(song?.summary.artist ?? "");
  const [album, setAlbum] = useState(song?.summary.album ?? "");
  if (!song) return null;
  return (
    <form
      class="stage__meta-form"
      onSubmit={(e) => {
        e.preventDefault();
        void saveMetadata({ title: title.trim(), artist: artist.trim(), album: album.trim() || null }).then((ok) => ok && onClose());
      }}
    >
      {/* The title field is the widest (style.css): a normal title fits without scrolling. */}
      <label class="stage__meta-field stage__meta-field--title">
        <span class="stage__meta-label">Title</span>
        <input value={title} required onInput={(e) => setTitle((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <label class="stage__meta-field">
        <span class="stage__meta-label">Artist</span>
        <input value={artist} onInput={(e) => setArtist((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <label class="stage__meta-field">
        <span class="stage__meta-label">Album</span>
        <input value={album} onInput={(e) => setAlbum((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <div class="stage__meta-actions">
        <button type="submit" class="button button--primary">
          Save details
        </button>
        <button type="button" class="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    </form>
  );
}

export function Stage() {
  const song = useApp((s) => s.song);
  const st = useApp((s) => s.songSettings);
  const ui = useApp((s) => s.settings?.ui);
  const devOffset = useApp((s) => deviceOffsetMs(s.settings));
  const fullscreen = useApp((s) => s.fullscreen);
  const pos = usePosition(2);
  const [editing, setEditing] = useState(false);
  const practising = usePractice((p) => p.on);

  if (!song || !st || !ui) {
    // One centred group (heading, line, button), so the button sits under the words
    // instead of floating at the bottom of the empty stage.
    return (
      <div class="stage stage--empty">
        <div class="stage__welcome">
          <h2 class="stage__welcome-title">No song loaded</h2>
          <p class="stage__empty">Pick a song in the library to start singing.</p>
          <button type="button" class="button button--primary stage__welcome-button" onClick={() => go("library")}>
            Open the library
          </button>
        </div>
      </div>
    );
  }
  const s = song.summary;
  const offsetS = (st.lyrics_offset_ms + devOffset) / 1000;

  return (
    <div class={`stage${fullscreen ? " stage--fullscreen" : ""}`}>
      <div class="stage__info">
        <div class="stage__names">
          <h2 class="stage__title">{s.title}</h2>
          <p class="stage__artist">{s.artist}</p>
        </div>
        <p class="stage__key">{keyLabel(s.key, st.semitones)}</p>
        <p class="stage__time">
          {formatTime(pos)} / {formatTime(s.duration_s)}
        </p>
        <div class="stage__tools">
          <button
            type="button"
            class="button button--small stage__practice"
            aria-pressed={practising}
            onClick={() => togglePractice()}
            title={practising ? "Leave practice mode (P)" : "Practice: sing into your microphone and see your pitch on the notes (P)"}
          >
            Practice
          </button>
          {practising && <PracticeTools />}
          {!fullscreen && !practising && (
            <>
              <button type="button" class="button button--small" onClick={() => setEditing((v) => !v)} aria-expanded={editing}>
                Edit details
              </button>
              <button type="button" class="button button--small" disabled={!song.lyrics} onClick={() => go("sync")}>
                Fix lyrics timing
              </button>
            </>
          )}
          <button
            type="button"
            class="button button--small stage__fullscreen"
            onClick={() => void setFullscreen(!fullscreen)}
            aria-label={fullscreen ? "Leave full screen (Esc)" : "Full screen (F)"}
            title={fullscreen ? "Leave full screen (Esc)" : "Full screen (F)"}
          >
            {fullscreen ? "Leave full screen" : "Full screen"}
          </button>
        </div>
      </div>
      {editing && <MetadataEditor onClose={() => setEditing(false)} />}
      {song.lyrics && !song.lyrics.checked && !practising && (
        <p class="stage__unchecked" role="note">
          These lyrics were {song.lyrics.source === "transcribed" ? "transcribed by the computer" : "added automatically"} and nobody has checked them yet. You can correct them with “Fix lyrics timing”.
        </p>
      )}
      {practising ? (
        <PracticeView song={song} lyricsOffsetS={offsetS} deviceOffsetS={devOffset / 1000} />
      ) : (
      <div class="stage__lane">
        {song.lyrics && song.lyrics.lines.length > 0 ? (
          ui.lyrics_display === "scroll" ? (
            <Ticker lyrics={song.lyrics} offsetS={offsetS} textSize={ui.lyrics_text_size} marker={ui.marker_position} pace={ui.lyrics_motion ?? "word"} colors={ui.colors} />
          ) : (
            <PageLyrics lyrics={song.lyrics} offsetS={offsetS} textSize={ui.lyrics_text_size} pace={ui.lyrics_motion ?? "word"} showNext={ui.show_next_line ?? true} showMarker={ui.show_marker ?? true} colors={ui.colors} />
          )
        ) : (
          <p class="stage__empty">This song has no lyrics (an instrumental). Sing along by ear, or add lyrics in the Studio.</p>
        )}
      </div>
      )}
    </div>
  );
}
