// SPDX-License-Identifier: GPL-3.0-or-later
// Library: the songs on the stick as jukebox title-strip cards (docs/DESIGN.md
// "Library"), with search, sort, cover art and the free space on the drive. Damaged
// or too-new files are listed with their error and cannot be opened.
//
// Markup hooks for the skin (class names are part of the UI's contract with
// app/ui/src/skin): .library, .library__toolbar, .library__search, .library__sort,
// .library__space, .title-strip-rack, .title-strip, .title-strip--current,
// .title-strip--damaged, .title-strip__code, .title-strip__cover, .title-strip__text,
// .title-strip__title, .title-strip__artist, .title-strip__meta, .title-strip__key,
// .title-strip__duration, .title-strip__badges, .title-strip__badge, .title-strip__error.
// With the skin on (`<Library skin />`) the songs are src/skin TitleStripCards in a
// TitleStripRack instead: .library--skin, .library__scroll, .library__strip,
// .library__strip--current, .library__strip--damaged, .jk-strip.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { LibrarySort, SongSummary } from "../api/types";
import { formatBytes, formatTime, selectionCode } from "../lib/format";
import { keyShort, pitchClass, signed } from "../lib/keys";
import { changeSettings, getApiSync, loadSong, refreshLibrary, useApp } from "../state/app";
import { Select } from "./common";
import { PlateButton, TitleStripCard, TitleStripRack } from "../skin";

/** Case- and accent-insensitive text for searching ("Frère" matches "frere"). */
export function fold(s: string): string {
  return s.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

export function sortSongs(songs: SongSummary[], by: LibrarySort): SongSummary[] {
  const coll = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
  const byTitle = (a: SongSummary, b: SongSummary) => coll.compare(a.title, b.title) || coll.compare(a.artist, b.artist);
  const out = [...songs];
  switch (by) {
    case "title":
      return out.sort(byTitle);
    case "artist":
      return out.sort((a, b) => coll.compare(a.artist, b.artist) || byTitle(a, b));
    case "recent":
      return out.sort((a, b) => b.modified_ms - a.modified_ms || byTitle(a, b));
    case "key": {
      const k = (s: SongSummary) => (s.key ? pitchClass(s.key.tonic) * 2 + (s.key.mode === "minor" ? 1 : 0) : 99);
      return out.sort((a, b) => k(a) - k(b) || byTitle(a, b));
    }
  }
}

export function filterSongs(songs: SongSummary[], query: string): SongSummary[] {
  const q = fold(query.trim());
  if (!q) return songs;
  const terms = q.split(/\s+/);
  return songs.filter((s) => {
    const hay = fold(`${s.title} ${s.artist} ${s.album ?? ""}`);
    return terms.every((t) => hay.includes(t));
  });
}

// Cover thumbnails are cached for the session (the core caches them too).
const coverCache = new Map<string, string | null>();

/**
 * The song's cover as a data URL, fetched once the element `ref` is (nearly) on
 * screen. Undefined while unknown, null when the song has no cover.
 */
function useCover(song: SongSummary, ref: { current: Element | null }, px: number): string | null | undefined {
  const [url, setUrl] = useState<string | null | undefined>(coverCache.get(song.song_id));
  useEffect(() => {
    if (url !== undefined || song.error) return;
    const el = ref.current;
    if (!el) return;
    let cancelled = false;
    const load = () => {
      getApiSync()
        .songCover(song.song_id, Math.round(px * (window.devicePixelRatio || 1)))
        .then((r) => {
          coverCache.set(song.song_id, r.data_url);
          if (!cancelled) setUrl(r.data_url);
        })
        .catch(() => !cancelled && setUrl(null));
    };
    // Only fetch covers that are (nearly) on screen.
    if (typeof IntersectionObserver === "undefined") {
      load();
      return () => {
        cancelled = true;
      };
    }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        load();
      }
    }, { rootMargin: "200px" });
    io.observe(el);
    return () => {
      cancelled = true;
      io.disconnect();
    };
  }, [song.song_id, url, song.error]);
  return url;
}

function Cover({ song }: { song: SongSummary }) {
  const ref = useRef<HTMLDivElement>(null);
  const url = useCover(song, ref, 72);
  return (
    <div class="title-strip__cover" ref={ref} aria-hidden="true">
      {url ? <img src={url} alt="" width={72} height={72} loading="lazy" decoding="async" /> : <span class="title-strip__cover-blank">♪</span>}
    </div>
  );
}

/** Short notes about a song for the skinned title strip (the plain card shows them as badges). */
function stripNote(song: SongSummary): string | undefined {
  if (song.error) return song.error;
  const notes: string[] = [];
  if (!song.has_lyrics) notes.push("No lyrics");
  else if (!song.lyrics_checked) notes.push("Lyrics not checked");
  if (song.semitones !== 0) notes.push(`Key ${signed(song.semitones)}`);
  if (song.preset === "clean") notes.push("Clean");
  return notes.length ? notes.join(" · ") : undefined;
}

/**
 * The same song as a jukebox title strip (skin on, src/skin TitleStripCard). The
 * wrapper (.library__strip, --current, --damaged, data-song-id) is what tests and
 * scripts look for; the button inside is .jk-strip.
 */
function SkinCard({ song, code, current }: { song: SongSummary; code: string; current: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const damaged = song.error !== null;
  // The strip's cover box is about 5vh square; ask for the size it is drawn at.
  const url = useCover(song, ref, Math.max(48, Math.round(window.innerHeight * 0.06)));
  return (
    <div
      ref={ref}
      class={`library__strip${current ? " library__strip--current" : ""}${damaged ? " library__strip--damaged" : ""}`}
      data-song-id={song.song_id}
    >
      <TitleStripCard
        title={song.title || "Untitled"}
        artist={song.artist || "Unknown artist"}
        keyName={song.key ? keyShort(song.key, song.semitones) : null}
        duration={damaged ? "" : song.duration_s}
        code={code}
        cover={url ?? null}
        selected={current}
        disabled={damaged}
        note={stripNote(song)}
        onSelect={() => void loadSong(song.song_id)}
      />
    </div>
  );
}

function Card({ song, code, current }: { song: SongSummary; code: string; current: boolean }) {
  const damaged = song.error !== null;
  const body = (
    <>
      <span class="title-strip__code">{code}</span>
      <Cover song={song} />
      <span class="title-strip__text">
        <span class="title-strip__title">{song.title || "Untitled"}</span>
        <span class="title-strip__artist">{song.artist || "Unknown artist"}</span>
        {damaged ? (
          <span class="title-strip__error" role="note">
            {song.error}
          </span>
        ) : (
          <span class="title-strip__badges">
            {!song.has_lyrics && <span class="title-strip__badge">No lyrics</span>}
            {song.has_lyrics && !song.lyrics_checked && <span class="title-strip__badge title-strip__badge--warn">Lyrics not checked</span>}
            {song.semitones !== 0 && <span class="title-strip__badge">Key {signed(song.semitones)}</span>}
            {song.preset === "clean" && <span class="title-strip__badge">Clean</span>}
          </span>
        )}
      </span>
      <span class="title-strip__meta">
        {song.key && (
          <span class="title-strip__key" title={song.semitones ? `Original ${song.key.name}, saved ${signed(song.semitones)}` : song.key.name}>
            {keyShort(song.key, song.semitones)}
          </span>
        )}
        {!damaged && <span class="title-strip__duration">{formatTime(song.duration_s)}</span>}
      </span>
    </>
  );
  if (damaged) {
    return (
      <div class="title-strip title-strip--damaged" aria-disabled="true">
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      class={`title-strip${current ? " title-strip--current" : ""}`}
      aria-current={current ? "true" : undefined}
      aria-label={`${song.title} by ${song.artist}, ${formatTime(song.duration_s)}${song.key ? `, ${keyShort(song.key, song.semitones)}` : ""}${
        !song.has_lyrics ? ", no lyrics" : !song.lyrics_checked ? ", lyrics not checked" : ""
      }. Sing this song.`}
      onClick={() => void loadSong(song.song_id)}
    >
      {body}
    </button>
  );
}

const SORTS: { value: LibrarySort; label: string }[] = [
  { value: "title", label: "Title" },
  { value: "artist", label: "Artist" },
  { value: "recent", label: "Recently added" },
  { value: "key", label: "Key" },
];

/**
 * The library screen. `skin` draws the same songs as jukebox title strips in a
 * chrome rack, with plate buttons for sorting and a black-glass search box
 * (docs/DESIGN.md "Library"); the behaviour (search, sort, Ctrl+F, Enter opens the
 * first match, damaged files listed but not playable) is the same in both looks.
 */
export function Library({ skin = false }: { skin?: boolean }) {
  const library = useApp((s) => s.library);
  const loading = useApp((s) => s.libraryLoading);
  const sort = useApp((s) => s.settings?.ui.library_sort ?? "title");
  const currentId = useApp((s) => s.song?.summary.song_id ?? null);
  const libraryPath = useApp((s) => s.info?.paths.library ?? "");
  const searchFocus = useApp((s) => s.searchFocus);
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (searchFocus > 0) {
      searchRef.current?.focus();
      searchRef.current?.select();
    }
  }, [searchFocus]);

  const songs = useMemo(() => sortSongs(library?.songs ?? [], sort), [library, sort]);
  const shown = useMemo(() => filterSongs(songs, query), [songs, query]);
  const codes = useMemo(() => new Map(songs.map((s, i) => [s.song_id, selectionCode(i)])), [songs]);
  const playable = songs.filter((s) => !s.error).length;

  const search = (
    <div class="field field--search library__search">
      <label for="library-search" class="visually-hidden">
        Search songs
      </label>
      <input
        id="library-search"
        ref={searchRef}
        class={skin ? "jk-glass-input" : undefined}
        type="search"
        placeholder="Search title, artist or album (Ctrl+F)"
        value={query}
        onInput={(e) => setQuery((e.currentTarget as HTMLInputElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setQuery("");
            (e.currentTarget as HTMLInputElement).blur();
          } else if (e.key === "Enter" && shown.length > 0 && !shown[0].error) {
            void loadSong(shown[0].song_id);
          }
        }}
      />
    </div>
  );
  const space = library && (
    <p class="library__space">
      {playable} song{playable === 1 ? "" : "s"} · <strong>{formatBytes(library.free_bytes)}</strong> free of {formatBytes(library.total_bytes)}
    </p>
  );
  const empty = (
    <>
      {library && songs.length === 0 && (
        <div class="library__empty">
          <p>No songs yet.</p>
          <p>
            Put <code>.jksong</code> files in <code>{libraryPath}</code>, or make one in the Studio on a Windows computer with an NVIDIA graphics card.
          </p>
        </div>
      )}
      {songs.length > 0 && shown.length === 0 && <p class="library__empty">No song matches “{query}”.</p>}
    </>
  );

  if (skin) {
    return (
      <div class="library library--skin">
        <div class="library__toolbar">
          {search}
          <div class="library__sort library__sort--plates" role="group" aria-label="Sort by">
            {SORTS.map((o) => (
              <PlateButton
                key={o.value}
                size="sm"
                label={o.value === "recent" ? "Recent" : o.label}
                title={`Sort by ${o.label.toLowerCase()}`}
                active={sort === o.value}
                onClick={() => changeSettings({ ui: { library_sort: o.value } })}
              />
            ))}
          </div>
          <PlateButton size="sm" class="library__rescan" label={loading ? "Reading…" : "Rescan"} title="Read the Library folder again" disabled={loading} onClick={() => void refreshLibrary()} />
          {space}
        </div>
        <div class="library__scroll">
          {empty}
          {shown.length > 0 && (
            <TitleStripRack label="Songs">
              {shown.map((s) => (
                <SkinCard key={s.song_id} song={s} code={codes.get(s.song_id) ?? ""} current={s.song_id === currentId} />
              ))}
            </TitleStripRack>
          )}
        </div>
      </div>
    );
  }

  return (
    <div class="library">
      <div class="library__toolbar">
        {search}
        <div class="library__sort">
          <Select<LibrarySort> label="Sort by" value={sort} options={SORTS} onChange={(v) => changeSettings({ ui: { library_sort: v } })} />
        </div>
        <button type="button" class="button library__rescan" onClick={() => void refreshLibrary()} disabled={loading}>
          {loading ? "Reading…" : "Rescan"}
        </button>
        {space}
      </div>

      {empty}

      <ul class="title-strip-rack" aria-label="Songs">
        {shown.map((s) => (
          <li key={s.song_id} class="title-strip-rack__slot">
            <Card song={s} code={codes.get(s.song_id) ?? ""} current={s.song_id === currentId} />
          </li>
        ))}
      </ul>
    </div>
  );
}
