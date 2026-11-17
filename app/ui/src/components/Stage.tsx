// SPDX-License-Identifier: GPL-3.0-or-later
// Stage: the singing view. Title, artist, key and time above the lyrics ticker;
// full-screen mode shows only this. Class hooks: .stage, .stage--fullscreen,
// .stage__info, .stage__title, .stage__artist, .stage__key, .stage__time,
// .stage__lane, .stage__empty, .stage__unchecked, .stage__tools.
import { useState } from "preact/hooks";
import { formatTime } from "../lib/format";
import { keyLabel } from "../lib/keys";
import { deviceOffsetMs, go, saveMetadata, setFullscreen, useApp } from "../state/app";
import { Ticker } from "../ticker/Ticker";
import { usePosition } from "./Transport";

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
      <label>
        Title <input value={title} required onInput={(e) => setTitle((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <label>
        Artist <input value={artist} onInput={(e) => setArtist((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <label>
        Album <input value={album} onInput={(e) => setAlbum((e.currentTarget as HTMLInputElement).value)} />
      </label>
      <button type="submit" class="button button--primary">
        Save details
      </button>
      <button type="button" class="button" onClick={onClose}>
        Cancel
      </button>
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

  if (!song || !st || !ui) {
    return (
      <div class="stage stage--empty">
        <p class="stage__empty">Pick a song in the library to start singing.</p>
        <button type="button" class="button button--primary" onClick={() => go("library")}>
          Open the library
        </button>
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
          {!fullscreen && (
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
      {song.lyrics && !song.lyrics.checked && (
        <p class="stage__unchecked" role="note">
          These lyrics were {song.lyrics.source === "transcribed" ? "transcribed by the computer" : "added automatically"} and nobody has checked them yet. You can correct them with “Fix lyrics timing”.
        </p>
      )}
      <div class="stage__lane">
        {song.lyrics && song.lyrics.lines.length > 0 ? (
          <Ticker lyrics={song.lyrics} offsetS={offsetS} textSize={ui.lyrics_text_size} marker={ui.marker_position} colors={ui.colors} />
        ) : (
          <p class="stage__empty">This song has no lyrics (an instrumental). Sing along by ear, or add lyrics in the Studio.</p>
        )}
      </div>
    </div>
  );
}
