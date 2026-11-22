// SPDX-License-Identifier: GPL-3.0-or-later
// Library: the songs on the stick as jukebox title-strip cards (docs/DESIGN.md
// "Library"), with search, sort, cover art and the free space on the drive. Damaged
// or too-new files are listed with their error and cannot be opened.
//
// Deleting songs (the owner's request of 2026-11-18, DECISIONS D-230): a trash plate
// on the hovered or current card, the Delete key on the focused or current card, and
// "Select" to tick several cards and delete them together. Every delete asks first; a
// deleted song moves to "Deleted songs" (the second view of this screen), where it is
// restored or deleted for good, each "for good" asking again because it cannot be undone.
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
// Deleting: .library__trash (a card's delete control), .library__views (Songs /
// Deleted songs), .library__select-bar, .library__strip--ticked, .title-strip--ticked,
// .deleted, .deleted-song, .confirm (the dialog), .confirm__cancel, .confirm__ok.
//
// Practice data (Work Order 2, APP_API 12.3: the melody guide practice mode draws and
// grades against): each song says whether practice is ready (a badge on the plain card,
// a note on the skinned strip; .title-strip__badge--practice, data-practice on the row),
// a song without it offers "Add practice data" (.library__practice-add), and the toolbar
// offers "Add practice data to all songs" (.library__practice-all) with its progress
// (.library__practice-progress) and a Stop control (.library__practice-stop). The full
// practice screen comes later (WO2 Stage 4).
import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { DeletedSong, GuideStatus, LibrarySort, SongSummary } from "../api/types";
import { formatBytes, formatTime, selectionCode } from "../lib/format";
import { keyShort, pitchClass, signed } from "../lib/keys";
import {
  buildAllGuides,
  buildGuide,
  cancelGuides,
  changeSettings,
  deleteSongs,
  guideNeedsAdding,
  getApiSync,
  loadSong,
  purgeSongs,
  refreshDeleted,
  refreshLibrary,
  restoreSongs,
  setModal,
  songLabel,
  store,
  useApp,
} from "../state/app";
import { Select } from "./common";
import { PlateButton, TitleStripCard, TitleStripRack, TransportIcon } from "../skin";

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

export function filterSongs<T extends { title: string; artist: string; album: string | null }>(songs: T[], query: string): T[] {
  const q = fold(query.trim());
  if (!q) return songs;
  const terms = q.split(/\s+/);
  return songs.filter((s) => {
    const hay = fold(`${s.title} ${s.artist} ${s.album ?? ""}`);
    return terms.every((t) => hay.includes(t));
  });
}

/** "18 Nov 2026, 21:30" in the computer's own language and time zone. */
export function formatDeletedAt(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  try {
    return new Date(ms).toLocaleString(undefined, { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });
  } catch {
    return new Date(ms).toISOString().slice(0, 16).replace("T", " ");
  }
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

/** What the practice badge says (null: an older core that reports no practice data). */
export function practiceBadge(g: GuideStatus | undefined): { text: string; kind: "ready" | "not-ready" | "busy"; title: string } | null {
  if (!g) return null;
  const graded = g.counts ? ` Graded on ${g.counts.graded} of ${g.counts.notes} notes.` : "";
  switch (g.state) {
    case "ready":
    case "outdated":
    case "newer":
      return {
        text: "Practice ready",
        kind: "ready",
        title: `This song has its practice data.${graded}${g.pending_write ? " It is saved into the song when the song stops playing." : ""}`,
      };
    case "queued":
      return { text: "Practice data waiting", kind: "busy", title: "Practice data will be added to this song shortly." };
    case "building":
      return { text: `Adding practice data ${Math.round((g.progress ?? 0) * 100)} %`, kind: "busy", title: "Practice data is being added to this song." };
    case "failed":
      if (g.in_memory) return { text: "Practice ready (this session)", kind: "ready", title: g.reason ?? "" };
      return { text: g.error_code === "guide_no_lead" ? "No practice (no lead vocal)" : "Practice not ready", kind: "not-ready", title: g.reason ?? "Practice data could not be added." };
    default:
      return { text: "Practice not ready", kind: "not-ready", title: g.reason ?? "Practice data has not been added to this song yet." };
  }
}

/** Short notes about a song for the skinned title strip (the plain card shows them as badges). */
function stripNote(song: SongSummary): string | undefined {
  if (song.error) return song.error;
  const notes: string[] = [];
  if (!song.has_lyrics) notes.push("No lyrics");
  else if (!song.lyrics_checked) notes.push("Lyrics not checked");
  if (song.semitones !== 0) notes.push(`Key ${signed(song.semitones)}`);
  if (song.preset === "clean") notes.push("Clean");
  const practice = practiceBadge(song.guide);
  if (practice) notes.push(practice.text);
  return notes.length ? notes.join(" · ") : undefined;
}

/** "Add practice data" for one song: shown while practice is not ready for it. */
function AddPracticeButton({ song, skin }: { song: SongSummary; skin: boolean }) {
  const g = song.guide;
  if (song.error || !guideNeedsAdding(g)) return null;
  const retry = g?.state === "failed";
  const label = `${retry ? "Try again: add" : "Add"} practice data to ${songLabel(song)}`;
  const title = retry && g?.reason ? `${g.reason}` : "Add practice data to this song (made from its own lead vocal; nothing else in the file changes)";
  if (skin) {
    return <PlateButton size="sm" class="library__practice-add" label={retry ? "Practice: retry" : "+ Practice"} ariaLabel={label} title={title} onClick={() => void buildGuide(song.song_id)} />;
  }
  return (
    <button type="button" class="button button--small library__practice-add" aria-label={label} title={title} onClick={() => void buildGuide(song.song_id)}>
      {retry ? "Try again" : "Add practice data"}
    </button>
  );
}

/** What a card does in Select mode (undefined: not selecting). */
interface Picking {
  ticked: boolean;
  toggle: () => void;
}

/** The card's delete control: a small trash plate (skin) or icon button (plain). */
function TrashButton({ song, skin, onDelete }: { song: SongSummary; skin: boolean; onDelete: () => void }) {
  const label = `Delete ${songLabel(song)}`;
  if (skin) {
    return <PlateButton size="sm" class="library__trash" label={<TransportIcon name="trash" />} ariaLabel={label} title={`${label} (Delete key)`} onClick={onDelete} />;
  }
  return (
    <button type="button" class="icon-button library__trash title-strip__delete" aria-label={label} title={`${label} (Delete key)`} onClick={onDelete}>
      <TransportIcon name="trash" />
    </button>
  );
}

/**
 * The same song as a jukebox title strip (skin on, src/skin TitleStripCard). The
 * wrapper (.library__strip, --current, --damaged, data-song-id) is what tests and
 * scripts look for; the button inside is .jk-strip, the trash plate beside it
 * .library__trash.
 */
function SkinCard({ song, code, current, picking, onDelete }: { song: SongSummary; code: string; current: boolean; picking?: Picking; onDelete: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const damaged = song.error !== null;
  // The strip's cover box is about 5vh square; ask for the size it is drawn at.
  const url = useCover(song, ref, Math.max(48, Math.round(window.innerHeight * 0.06)));
  return (
    <div
      ref={ref}
      class={`library__strip${current ? " library__strip--current" : ""}${damaged ? " library__strip--damaged" : ""}${picking?.ticked ? " library__strip--ticked" : ""}${picking ? " library__strip--picking" : ""}${!picking && !damaged && guideNeedsAdding(song.guide) ? " library__strip--can-add" : ""}`}
      data-song-id={song.song_id}
      data-practice={damaged ? undefined : practiceBadge(song.guide)?.kind}
    >
      <TitleStripCard
        title={song.title || "Untitled"}
        artist={song.artist || "Unknown artist"}
        keyName={song.key ? keyShort(song.key, song.semitones) : null}
        duration={damaged ? "" : song.duration_s}
        code={code}
        cover={url ?? null}
        selected={!picking && current}
        disabled={damaged && !picking}
        note={stripNote(song)}
        checked={picking?.ticked}
        onSelect={() => (picking ? picking.toggle() : void loadSong(song.song_id))}
      />
      {!picking && (
        <div class="library__strip-actions">
          <AddPracticeButton song={song} skin />
          <TrashButton song={song} skin onDelete={onDelete} />
        </div>
      )}
    </div>
  );
}

function Card({ song, code, current, picking, onDelete }: { song: SongSummary; code: string; current: boolean; picking?: Picking; onDelete: () => void }) {
  const damaged = song.error !== null;
  const practice = damaged ? null : practiceBadge(song.guide);
  const body = (
    <>
      <span class="title-strip__code">
        {picking ? <span class={`title-strip__check${picking.ticked ? " title-strip__check--on" : ""}`} aria-hidden="true">{picking.ticked ? "✓" : ""}</span> : code}
      </span>
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
            {practice && (
              <span class={`title-strip__badge title-strip__badge--practice title-strip__badge--practice-${practice.kind}`} title={practice.title}>
                {practice.text}
              </span>
            )}
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
  let card;
  if (picking) {
    card = (
      <button
        type="button"
        role="checkbox"
        aria-checked={picking.ticked ? "true" : "false"}
        class={`title-strip title-strip--picking${picking.ticked ? " title-strip--ticked" : ""}${damaged ? " title-strip--damaged" : ""}`}
        aria-label={`${songLabel(song)}${damaged ? " (damaged)" : ""}`}
        onClick={picking.toggle}
      >
        {body}
      </button>
    );
  } else if (damaged) {
    card = (
      <div class="title-strip title-strip--damaged" aria-disabled="true" tabIndex={0}>
        {body}
      </div>
    );
  } else {
    card = (
      <button
        type="button"
        class={`title-strip${current ? " title-strip--current" : ""}`}
        aria-current={current ? "true" : undefined}
        aria-label={`${song.title} by ${song.artist}, ${formatTime(song.duration_s)}${song.key ? `, ${keyShort(song.key, song.semitones)}` : ""}${
          !song.has_lyrics ? ", no lyrics" : !song.lyrics_checked ? ", lyrics not checked" : ""
        }${practice ? `, ${practice.text.toLowerCase()}` : ""}. Sing this song.`}
        onClick={() => void loadSong(song.song_id)}
      >
        {body}
      </button>
    );
  }
  return (
    <div class="title-strip__row" data-song-id={song.song_id} data-practice={practice?.kind}>
      {card}
      {!picking && (
        <div class="title-strip__actions">
          <AddPracticeButton song={song} skin={false} />
          <TrashButton song={song} skin={false} onDelete={onDelete} />
        </div>
      )}
    </div>
  );
}

const SORTS: { value: LibrarySort; label: string }[] = [
  { value: "title", label: "Title" },
  { value: "artist", label: "Artist" },
  { value: "recent", label: "Recently added" },
  { value: "key", label: "Key" },
];

/** What the confirmation dialog is asking about. */
type Ask = { kind: "delete"; ids: string[] } | { kind: "purge"; ids: string[] } | { kind: "empty" };

/**
 * A question before something is deleted: a modal dialog (native <dialog>, so focus
 * stays inside), Cancel focused first, Escape or Cancel does nothing. With the skin on:
 * a chrome-framed black-glass panel with plate buttons, the confirming one lit red.
 */
export function ConfirmDialog(props: {
  title: string;
  children: ComponentChildren;
  confirmLabel: string;
  skin: boolean;
  /** Red "danger" OK button (default true). */
  danger?: boolean;
  /** OK button text while onConfirm runs (default "Deleting…"). */
  busyLabel?: string;
  onConfirm: () => Promise<unknown> | void;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const cancel = useRef(props.onCancel);
  cancel.current = props.onCancel;
  useEffect(() => {
    setModal(true);
    const d = ref.current;
    if (!d) return () => setModal(false);
    if (typeof d.showModal === "function" && !d.open) {
      try {
        d.showModal();
      } catch {
        d.setAttribute("open", "");
      }
    } else d.setAttribute("open", "");
    // Cancel first: Enter or Space without looking deletes nothing.
    d.querySelector<HTMLButtonElement>(".confirm__cancel")?.focus();
    const onCancelEvent = (e: Event) => {
      e.preventDefault();
      cancel.current();
    };
    d.addEventListener("cancel", onCancelEvent);
    return () => {
      d.removeEventListener("cancel", onCancelEvent);
      if (d.open && typeof d.close === "function") d.close();
      setModal(false);
    };
  }, []);
  const confirm = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await props.onConfirm();
    } finally {
      setBusy(false);
    }
  };
  const buttons = props.skin ? (
    <>
      <PlateButton class="confirm__cancel" label="Cancel" disabled={busy} onClick={() => props.onCancel()} />
      <PlateButton class={`confirm__ok${props.danger === false ? "" : " jk-plate--danger"}`} label={busy ? (props.busyLabel ?? "Deleting…") : props.confirmLabel} disabled={busy} onClick={() => void confirm()} />
    </>
  ) : (
    <>
      <button type="button" class="button confirm__cancel" disabled={busy} onClick={() => props.onCancel()}>
        Cancel
      </button>
      <button type="button" class={`button ${props.danger === false ? "button--primary" : "button--danger"} confirm__ok`} disabled={busy} onClick={() => void confirm()}>
        {busy ? (props.busyLabel ?? "Deleting…") : props.confirmLabel}
      </button>
    </>
  );
  return (
    <dialog
      ref={ref}
      class={`dialog confirm${props.skin ? " confirm--skin" : ""}`}
      aria-labelledby="confirm-title"
      aria-describedby="confirm-body"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          props.onCancel();
        }
      }}
    >
      <div class="confirm__face">
        <h2 id="confirm-title" class="confirm__title">
          {props.title}
        </h2>
        <div id="confirm-body" class="confirm__body">
          {props.children}
        </div>
        <div class="confirm__actions">{buttons}</div>
      </div>
    </dialog>
  );
}

/** The text and action of each question (D-230's wording). */
function AskDialog({ ask, skin, onDone }: { ask: Ask; skin: boolean; onDone: (confirmed: boolean) => void }) {
  const library = useApp((s) => s.library);
  const deleted = useApp((s) => s.deleted);
  const song = useApp((s) => s.song);
  const playing = useApp((s) => s.player?.state === "playing");
  const run = (p: Promise<boolean>) => p.then((ok) => onDone(ok));
  const cancel = () => onDone(false);
  if (ask.kind === "delete") {
    const songs = ask.ids.map((id) => library?.songs.find((s) => s.song_id === id)).filter((s): s is SongSummary => !!s);
    const one = songs.length === 1 ? songs[0] : null;
    const loaded = song && ask.ids.includes(song.summary.song_id);
    return (
      <ConfirmDialog
        skin={skin}
        title={one ? `Delete “${songLabel(one)}”?` : `Delete ${ask.ids.length} songs?`}
        confirmLabel={one ? "Delete" : `Delete ${ask.ids.length} songs`}
        onConfirm={() => run(deleteSongs(ask.ids))}
        onCancel={cancel}
      >
        <p>{one ? "It moves to Deleted songs, where you can restore it or delete it for good." : "They move to Deleted songs, where you can restore them or delete them for good."}</p>
        {!one && songs.length > 0 && (
          <ul class="confirm__list">
            {songs.slice(0, 6).map((s) => (
              <li key={s.song_id}>{songLabel(s)}</li>
            ))}
            {songs.length > 6 && <li>and {songs.length - 6} more</li>}
          </ul>
        )}
        {loaded && <p class="confirm__note">{one ? "It is" : "One of them is"} {playing ? "playing" : "open"} now; it will stop.</p>}
      </ConfirmDialog>
    );
  }
  const all = deleted?.songs ?? [];
  if (ask.kind === "purge") {
    const songs = ask.ids.map((id) => all.find((s) => s.deleted_id === id)).filter((s): s is DeletedSong => !!s);
    const one = songs.length === 1 ? songs[0] : null;
    const bytes = songs.reduce((t, s) => t + s.file_bytes, 0);
    return (
      <ConfirmDialog
        skin={skin}
        title={one ? `Delete “${songLabel(one)}” for good?` : `Delete ${ask.ids.length} songs for good?`}
        confirmLabel="Delete for good"
        onConfirm={() => run(purgeSongs(ask.ids))}
        onCancel={cancel}
      >
        <p>
          <strong>This cannot be undone.</strong> {one ? "The song file is" : "The song files are"} removed from the drive, freeing {formatBytes(bytes)}.
        </p>
      </ConfirmDialog>
    );
  }
  const bytes = all.reduce((t, s) => t + s.file_bytes, 0);
  return (
    <ConfirmDialog skin={skin} title="Empty Deleted songs?" confirmLabel="Empty Deleted songs" onConfirm={() => run(purgeSongs("all"))} onCancel={cancel}>
      <p>
        <strong>This cannot be undone.</strong> {all.length === 1 ? "The song" : `All ${all.length} songs`} in Deleted songs ({formatBytes(bytes)}) {all.length === 1 ? "is" : "are"} deleted for good.
      </p>
    </ConfirmDialog>
  );
}

/** "Deleted songs": what was deleted, when, how big; Restore and Delete for good. */
function DeletedView({ skin, query, onAsk }: { skin: boolean; query: string; onAsk: (a: Ask) => void }) {
  const deleted = useApp((s) => s.deleted);
  useEffect(() => {
    void refreshDeleted();
  }, []);
  const songs = useMemo(() => filterSongs(deleted?.songs ?? [], query), [deleted, query]);
  const plate = (label: string, title: string, onClick: () => void, cls: string, danger = false) =>
    skin ? (
      <PlateButton size="sm" class={`${cls}${danger ? " jk-plate--danger-hover" : ""}`} label={label} title={title} onClick={onClick} />
    ) : (
      <button type="button" class={`button ${cls}${danger ? " button--danger" : ""}`} title={title} onClick={onClick}>
        {label}
      </button>
    );
  const rows = songs.map((s) => (
    <li key={s.deleted_id} class="deleted-song" data-deleted-id={s.deleted_id}>
      <span class="deleted-song__tab" aria-hidden="true">
        <TransportIcon name="trash" />
      </span>
      <span class="deleted-song__text">
        <span class="deleted-song__title">{s.title || "Untitled"}</span>
        <span class="deleted-song__artist">{s.artist || "Unknown artist"}</span>
        <span class="deleted-song__from" title={s.restore_to !== s.original_song_id ? `Its place is taken now, so it comes back as ${s.restore_to}` : undefined}>
          {s.restore_to === s.original_song_id ? `From ${s.original_song_id}` : `From ${s.original_song_id} (comes back as ${s.restore_to})`}
        </span>
      </span>
      <span class="deleted-song__meta">
        <span class="deleted-song__size">{formatBytes(s.file_bytes)}</span>
        <span class="deleted-song__date">Deleted {formatDeletedAt(s.deleted_ms)}</span>
      </span>
      <span class="deleted-song__actions">
        {plate("Restore", `Put ${songLabel(s)} back in the library`, () => void restoreSongs([s.deleted_id]), "deleted-song__restore")}
        {plate("Delete for good", `Delete ${songLabel(s)} for good (asks first)`, () => onAsk({ kind: "purge", ids: [s.deleted_id] }), "deleted-song__purge", true)}
      </span>
    </li>
  ));
  const count = deleted?.count ?? 0;
  return (
    <section class={`deleted${skin ? " deleted--skin" : ""}`} aria-label="Deleted songs">
      <div class="deleted__header">
        <h2 class="deleted__title">Deleted songs</h2>
        <p class="deleted__summary">
          {count === 0 ? "Empty" : `${count} song${count === 1 ? "" : "s"} · ${formatBytes(deleted?.total_bytes ?? 0)} still on the drive`}
        </p>
        {skin ? (
          <PlateButton size="sm" class="deleted__empty-all jk-plate--danger-hover" label="Empty Deleted songs" title="Delete every song here for good (asks first)" disabled={count === 0} onClick={() => onAsk({ kind: "empty" })} />
        ) : (
          <button type="button" class="button button--danger deleted__empty-all" disabled={count === 0} onClick={() => onAsk({ kind: "empty" })}>
            Empty Deleted songs
          </button>
        )}
      </div>
      <p class="deleted__note">
        Deleted songs wait here, on the drive, until you restore them or delete them for good. Restore puts a song back where it was in the library.
      </p>
      {deleted && count === 0 && <p class="library__empty">No deleted songs.</p>}
      {count > 0 && songs.length === 0 && <p class="library__empty">No deleted song matches “{query}”.</p>}
      {songs.length > 0 &&
        (skin ? (
          <TitleStripRack label="Deleted songs" class="deleted__rack">
            <ul class="deleted__list">{rows}</ul>
          </TitleStripRack>
        ) : (
          <ul class="deleted__list">{rows}</ul>
        ))}
    </section>
  );
}

/** True when the key event is typing into a text field. */
function typingIn(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable || t instanceof HTMLTextAreaElement || t instanceof HTMLSelectElement) return true;
  return t instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit", "reset", "color", "range"].includes(t.type);
}

/**
 * The library screen. `skin` draws the same songs as jukebox title strips in a
 * chrome rack, with plate buttons for sorting and a black-glass search box
 * (docs/DESIGN.md "Library"); the behaviour (search, sort, Ctrl+F, Enter opens the
 * first match, damaged files listed but not playable, deleting) is the same in both looks.
 */
/**
 * "Add practice data to all songs", or its progress with a Stop control while it runs.
 * Hidden when every song already has practice data (or none can get it).
 */
function PracticeAll({ skin, songs }: { skin: boolean; songs: SongSummary[] }) {
  const batch = useApp((s) => s.guideBatch);
  const missing = songs.filter((s) => !s.error && guideNeedsAdding(s.guide)).length;
  if (batch?.running) {
    const text = `Adding practice data: ${Math.min(batch.done + 1, batch.total)} of ${batch.total}${batch.failed ? ` (${batch.failed} failed)` : ""}`;
    return (
      <div class="library__practice" role="group" aria-label="Practice data">
        <span class="library__practice-progress" aria-live="polite" role="status">
          {text}
        </span>
        {skin ? (
          <PlateButton size="sm" class="library__practice-stop" label="Stop" title="Stop adding practice data (the songs stay as they are)" onClick={() => void cancelGuides()} />
        ) : (
          <button type="button" class="button library__practice-stop" onClick={() => void cancelGuides()}>
            Stop
          </button>
        )}
      </div>
    );
  }
  if (missing === 0) return null;
  const title = `Add practice data to ${missing} song${missing === 1 ? "" : "s"} that have none yet (made from each song's own lead vocal; nothing else in the files changes)`;
  return skin ? (
    <PlateButton size="sm" class="library__practice-all" label="Add practice data" ariaLabel="Add practice data to all songs" title={title} onClick={() => void buildAllGuides()} />
  ) : (
    <button type="button" class="button library__practice-all" title={title} onClick={() => void buildAllGuides()}>
      Add practice data to all songs
    </button>
  );
}

export function Library({ skin = false }: { skin?: boolean }) {
  const library = useApp((s) => s.library);
  const loading = useApp((s) => s.libraryLoading);
  const sort = useApp((s) => s.settings?.ui.library_sort ?? "title");
  const currentId = useApp((s) => s.song?.summary.song_id ?? null);
  const libraryPath = useApp((s) => s.info?.paths.library ?? "");
  const searchFocus = useApp((s) => s.searchFocus);
  const deletedList = useApp((s) => s.deleted);
  const platform = useApp((s) => s.info?.platform ?? "windows");
  const [query, setQuery] = useState("");
  const [view, setView] = useState<"songs" | "deleted">("songs");
  const [picking, setPicking] = useState(false);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [ask, setAsk] = useState<Ask | null>(null);
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
  const deletedCount = deletedList?.count ?? library?.deleted_count ?? 0;

  // Ticks of songs that are gone (deleted, or removed by hand) are dropped.
  useEffect(() => {
    if (ticked.size === 0) return;
    const ids = new Set(songs.map((s) => s.song_id));
    if ([...ticked].some((id) => !ids.has(id))) setTicked(new Set([...ticked].filter((id) => ids.has(id))));
  }, [songs]);

  const stopPicking = () => {
    setPicking(false);
    setTicked(new Set());
  };
  const toggle = (id: string) =>
    setTicked((t) => {
      const n = new Set(t);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  // Delete (or Cmd+Backspace on a Mac) asks about the focused card, the ticked cards in
  // Select mode, or else the current song; Escape leaves Select mode.
  const keyState = useRef({ view, picking, ticked, shown, currentId, ask, platform });
  keyState.current = { view, picking, ticked, shown, currentId, ask, platform };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = keyState.current;
      const s = store.get();
      if (e.defaultPrevented || k.ask || s.modal || s.overlay || s.tapMode || s.screen !== "library" || s.fullscreen) return;
      if (typingIn(e.target)) return;
      if (e.key === "Escape") {
        if (k.picking) {
          e.preventDefault();
          stopPicking();
        }
        return;
      }
      const del = (e.key === "Delete" && !e.ctrlKey && !e.altKey && !e.metaKey) || (k.platform === "macos" && e.key === "Backspace" && e.metaKey);
      if (!del || k.view !== "songs") return;
      const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest("[data-song-id]")?.getAttribute("data-song-id") : null;
      const visible = new Set(k.shown.map((x) => x.song_id));
      let ids: string[] = [];
      if (k.picking) ids = [...k.ticked];
      else if (focused && visible.has(focused)) ids = [focused];
      else if (k.currentId && visible.has(k.currentId)) ids = [k.currentId];
      if (ids.length === 0) return;
      e.preventDefault();
      setAsk({ kind: "delete", ids });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const finishAsk = (confirmed: boolean) => {
    const was = ask;
    setAsk(null);
    if (confirmed && was?.kind === "delete") stopPicking();
  };

  const search = (
    <div class="field field--search library__search">
      <label for="library-search" class="visually-hidden">
        {view === "deleted" ? "Search deleted songs" : "Search songs"}
      </label>
      <input
        id="library-search"
        ref={searchRef}
        class={skin ? "jk-glass-input" : undefined}
        type="search"
        placeholder={view === "deleted" ? "Search deleted songs" : "Search title, artist or album (Ctrl+F)"}
        value={query}
        onInput={(e) => setQuery((e.currentTarget as HTMLInputElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            setQuery("");
            (e.currentTarget as HTMLInputElement).blur();
          } else if (e.key === "Enter" && view === "songs" && !picking && shown.length > 0 && !shown[0].error) {
            void loadSong(shown[0].song_id);
          }
        }}
      />
    </div>
  );
  const space = library && (
    <p class="library__space">
      {playable} song{playable === 1 ? "" : "s"} · <strong>{formatBytes(library.free_bytes)}</strong> free of {formatBytes(library.total_bytes)}
      {(library.deleted_count ?? 0) > 0 && (
        <span class="library__space-deleted" title="Deleted songs still take this space until they are deleted for good">
          {" "}
          · {library.deleted_count} deleted ({formatBytes(library.deleted_bytes ?? 0)})
        </span>
      )}
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

  // Songs / Deleted songs, and Select.
  const goView = (v: "songs" | "deleted") => {
    stopPicking();
    setView(v);
  };
  const viewSwitch = skin ? (
    <div class="library__views" role="group" aria-label="Show">
      <PlateButton size="sm" label="Songs" title="The songs in the library" active={view === "songs"} onClick={() => goView("songs")} />
      <PlateButton size="sm" class="library__show-deleted" label={`Deleted${deletedCount ? ` (${deletedCount})` : ""}`} title="Deleted songs: restore them or delete them for good" active={view === "deleted"} onClick={() => goView("deleted")} />
    </div>
  ) : (
    <div class="library__views" role="group" aria-label="Show">
      <button type="button" class="button" aria-pressed={view === "songs" ? "true" : "false"} onClick={() => goView("songs")}>
        Songs
      </button>
      <button type="button" class="button library__show-deleted" aria-pressed={view === "deleted" ? "true" : "false"} onClick={() => goView("deleted")}>
        Deleted songs{deletedCount ? ` (${deletedCount})` : ""}
      </button>
    </div>
  );
  const selectable = shown.map((s) => s.song_id);
  const allTicked = selectable.length > 0 && selectable.every((id) => ticked.has(id));
  const selectBar = picking && (
    <div class="library__select-bar" role="group" aria-label="Selected songs">
      <span class="library__select-count" aria-live="polite">
        {ticked.size === 0 ? "Tick the songs to delete" : `${ticked.size} selected`}
      </span>
      {skin ? (
        <>
          <PlateButton size="sm" class="library__select-all" label={allTicked ? "None" : "All"} title={allTicked ? "Untick every song" : "Tick every song shown"} onClick={() => setTicked(allTicked ? new Set() : new Set(selectable))} />
          <PlateButton size="sm" class="library__delete-ticked jk-plate--danger" label={`Delete ${ticked.size || ""}`.trim()} title="Delete the ticked songs (asks first)" disabled={ticked.size === 0} onClick={() => setAsk({ kind: "delete", ids: [...ticked] })} />
          <PlateButton size="sm" class="library__select-done" label="Done" title="Leave Select (Esc)" onClick={stopPicking} />
        </>
      ) : (
        <>
          <button type="button" class="button library__select-all" onClick={() => setTicked(allTicked ? new Set() : new Set(selectable))}>
            {allTicked ? "Select none" : "Select all"}
          </button>
          <button type="button" class="button button--danger library__delete-ticked" disabled={ticked.size === 0} onClick={() => setAsk({ kind: "delete", ids: [...ticked] })}>
            Delete{ticked.size ? ` ${ticked.size}` : ""}
          </button>
          <button type="button" class="button library__select-done" onClick={stopPicking}>
            Done
          </button>
        </>
      )}
    </div>
  );
  const selectToggle =
    view === "songs" &&
    !picking &&
    songs.length > 0 &&
    (skin ? (
      <PlateButton size="sm" class="library__select" label="Select" title="Tick several songs to delete them together" onClick={() => setPicking(true)} />
    ) : (
      <button type="button" class="button library__select" onClick={() => setPicking(true)}>
        Select
      </button>
    ));
  const pickFor = (s: SongSummary): Picking | undefined => (picking ? { ticked: ticked.has(s.song_id), toggle: () => toggle(s.song_id) } : undefined);
  const askDelete = (s: SongSummary) => setAsk({ kind: "delete", ids: [s.song_id] });
  const dialog = ask && <AskDialog ask={ask} skin={skin} onDone={finishAsk} />;

  if (skin) {
    return (
      <div class={`library library--skin${picking ? " library--picking" : ""}${view === "deleted" ? " library--deleted" : ""}`}>
        <div class="library__toolbar">
          {search}
          {view === "songs" && !picking && (
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
          )}
          {selectBar}
          {view === "songs" && !picking && (
            <PlateButton size="sm" class="library__rescan" label={loading ? "Reading…" : "Rescan"} title="Read the Library folder again" disabled={loading} onClick={() => void refreshLibrary()} />
          )}
          {view === "songs" && !picking && <PracticeAll skin songs={songs} />}
          {selectToggle}
          {viewSwitch}
          {space}
        </div>
        <div class="library__scroll">
          {view === "deleted" ? (
            <DeletedView skin query={query} onAsk={setAsk} />
          ) : (
            <>
              {empty}
              {shown.length > 0 && (
                <TitleStripRack label="Songs">
                  {shown.map((s) => (
                    <SkinCard key={s.song_id} song={s} code={codes.get(s.song_id) ?? ""} current={s.song_id === currentId} picking={pickFor(s)} onDelete={() => askDelete(s)} />
                  ))}
                </TitleStripRack>
              )}
            </>
          )}
        </div>
        {dialog}
      </div>
    );
  }

  return (
    <div class={`library${picking ? " library--picking" : ""}${view === "deleted" ? " library--deleted" : ""}`}>
      <div class="library__toolbar">
        {search}
        {view === "songs" && !picking && (
          <div class="library__sort">
            <Select<LibrarySort> label="Sort by" value={sort} options={SORTS} onChange={(v) => changeSettings({ ui: { library_sort: v } })} />
          </div>
        )}
        {view === "songs" && !picking && (
          <button type="button" class="button library__rescan" onClick={() => void refreshLibrary()} disabled={loading}>
            {loading ? "Reading…" : "Rescan"}
          </button>
        )}
        {view === "songs" && !picking && <PracticeAll skin={false} songs={songs} />}
        {selectBar}
        {selectToggle}
        {viewSwitch}
        {space}
      </div>

      {view === "deleted" ? (
        <DeletedView skin={false} query={query} onAsk={setAsk} />
      ) : (
        <>
          {empty}
          <ul class="title-strip-rack" aria-label="Songs">
            {shown.map((s) => (
              <li key={s.song_id} class="title-strip-rack__slot">
                <Card song={s} code={codes.get(s.song_id) ?? ""} current={s.song_id === currentId} picking={pickFor(s)} onDelete={() => askDelete(s)} />
              </li>
            ))}
          </ul>
        </>
      )}
      {dialog}
    </div>
  );
}
