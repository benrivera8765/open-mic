// SPDX-License-Identifier: GPL-3.0-or-later
// Studio panel (WORKING ORDER 4.1, STUDIO_PROTOCOL): drag-and-drop or pick files,
// inspect each one so the title, artist, album and cover picture can be corrected (the
// cover replaced by a JPEG / PNG or removed: `metadata.cover_path`), choose where the lyrics
// come from (paste, .lrc, automatic, none, or an online lookup the user asks for),
// pick the quality preset, then queue it. The queue shows every stage, and a file
// that fails shows its own error without stopping the others.
// Where the Studio cannot run, the panel says why in plain words.
// Class hooks: .studio, .studio__drop, .studio__drop--over, .studio__item,
// .studio__cover, .studio__cover-img, .studio__queue, .studio__job, .studio__job--failed, .studio__job--done,
// .studio__stages, .studio__stage, .studio__stage--done, .studio__stage--current.
import { useEffect, useState } from "preact/hooks";
import type { InspectResult, LookupResult, StudioJob, StudioLyricsMode, StudioModelsStatus, StudioPreset } from "../api/types";
import { STUDIO_STAGES } from "../api/types";
import { errorMessage } from "../api/client";
import { formatBytes, formatRate, formatTime } from "../lib/format";
import { addToStudio, getApiSync, loadSong, refreshStudio, removeFromStudio, toast, useApp } from "../state/app";
import { Note, Progress, Section, Select } from "./common";

const PRESETS: { value: StudioPreset; label: string; hint: string }[] = [
  { value: "full", label: "Full (recommended)", hint: "Removes only the lead singer from the original recording: nothing else is lost, a faint trace of the voice can remain." },
  { value: "clean", label: "Clean", hint: "A model rebuilds the music without the singer: less voice left over, sometimes a little less body." },
  // studio/jukeoke_studio/presets.py PRESET_INFO["permissive"]; the core already accepted it.
  { value: "permissive", label: "Permissive licences only", hint: "Only MIT / Apache-2.0 models. No such model can tell the lead singer from the backing singers, so every voice is removed: backing vocals and harmonies are missing." },
];

const STAGE_LABELS: Record<string, string> = { decode: "Read", separate: "Separate", lyrics: "Lyrics", align: "Align", analyse: "Key & tempo", write: "Save" };

const LANGUAGES = [
  { value: "en", label: "English" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
  { value: "es", label: "Spanish" },
  { value: "it", label: "Italian" },
  { value: "pt", label: "Portuguese" },
  { value: "nl", label: "Dutch" },
];

/**
 * The core and the Studio end every reason with "Songs are created on a stronger computer
 * and can be played anywhere." so each reason stands on its own (Settings, logs). The panel
 * says it once, in bold, above the list: drop it from the reasons and drop repeats (D-174).
 */
const TAGLINE = /\s*Songs are created on a stronger computer and (?:can be )?played anywhere\.?/gi;
export function reasonsForPanel(reasons: string[]): string[] {
  const out: string[] = [];
  for (const r of reasons) {
    const s = r.replace(TAGLINE, "").trim();
    if (s && !out.includes(s)) out.push(s);
  }
  return out;
}

function Unavailable({ reasons }: { reasons: string[] }) {
  const shown = reasonsForPanel(reasons);
  return (
    <Section title="Studio" class="studio studio--unavailable">
      <p>
        <strong>Songs are created on a stronger computer and can be played anywhere.</strong> The Studio turns an ordinary song into a karaoke song; it needs a Windows computer with an NVIDIA
        graphics card. Everything else (playing, changing the key, fixing lyrics timing, recording) works here.
      </p>
      {shown.length > 0 && (
        <ul class="studio__reasons">
          {shown.map((r) => (
            <li key={r}>{r}</li>
          ))}
        </ul>
      )}
    </Section>
  );
}

function Models() {
  const present = useApp((s) => s.studio?.models_present ?? true);
  const [status, setStatus] = useState<StudioModelsStatus | null>(null);
  const [progress, setProgress] = useState<{ name: string; done: number; total: number } | null>(null);
  useEffect(() => {
    if (present) return;
    void getApiSync().studioModelsStatus().then(setStatus).catch(() => undefined);
    return getApiSync().on("studio://models", (e) => {
      setProgress({ name: e.name, done: e.done_bytes, total: e.total_bytes });
      if (e.total_bytes > 0 && e.done_bytes >= e.total_bytes) void refreshStudio();
    });
  }, [present]);
  if (present) return null;
  const missing = status?.models.filter((m) => !m.present) ?? [];
  const bytes = missing.reduce((a, m) => a + m.size_bytes, 0);
  return (
    <Note kind="warn">
      The Studio's models are not on this drive yet{bytes ? ` (${formatBytes(bytes)} to download, once)` : ""}. They are only downloaded when you ask.{" "}
      {progress ? (
        <Progress value={progress.total ? progress.done / progress.total : 0} label={`Downloading ${progress.name}`} />
      ) : (
        <button type="button" class="button button--small" onClick={() => void getApiSync().studioModelsDownload().catch((e) => toast(errorMessage(e), "error"))}>
          Download models
        </button>
      )}
      {missing.length > 0 && (
        <span class="studio__models">
          {missing.map((m) => `${m.name} (${m.licence}${m.commercial === "yes" ? "" : ", non-commercial"})`).join(" · ")}
        </span>
      )}
    </Note>
  );
}

const COVER_PREVIEW_PX = 192;

/**
 * The cover in the form (WORKING ORDER 4.1 step 1): the one read from the file, or the
 * picture the user chose instead, or none. `choice` undefined = the file's own cover
 * (nothing is sent), a path = that picture, null = removed (STUDIO_PROTOCOL 7
 * `metadata.cover_path`).
 */
function CoverField({ id, fromFile, choice, onChoice }: { id: string; fromFile: string | null; choice: string | null | undefined; onChoice: (c: string | null | undefined) => void }) {
  const api = getApiSync();
  const shown = choice === undefined ? fromFile : choice;
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    setUrl(null);
    if (!shown || !api.studioCoverPreview) return;
    let cancelled = false;
    api
      .studioCoverPreview(shown, COVER_PREVIEW_PX)
      .then((r) => !cancelled && setUrl(r.data_url))
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [shown]);
  const pick = async () => {
    try {
      const p = await api.pickCoverImage!();
      if (!p) return;
      await api.studioCoverPreview?.(p, COVER_PREVIEW_PX); // refuses anything but JPEG / PNG
      onChoice(p);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };
  const label = choice === undefined ? (fromFile ? "From the file" : "The file has no cover") : choice === null ? "No cover" : `Your picture: ${choice.split(/[\\/]/).pop()}`;
  return (
    <>
      <span id={id}>Cover</span>
      <div class="studio__cover row" role="group" aria-labelledby={id}>
        {shown && url ? (
          <img class="studio__cover-img" src={url} alt="Cover picture" width={96} height={96} style={{ objectFit: "cover", borderRadius: "var(--radius)" }} />
        ) : null}
        <span class="hint">{label}</span>
        {api.pickCoverImage && (
          <button type="button" class="button button--small" onClick={() => void pick()}>
            {shown ? "Replace cover…" : "Choose a cover picture…"}
          </button>
        )}
        {shown && (
          <button type="button" class="button button--small" onClick={() => onChoice(null)}>
            Remove cover
          </button>
        )}
        {choice !== undefined && fromFile && (
          <button type="button" class="button button--small" onClick={() => onChoice(undefined)}>
            Use the file's cover
          </button>
        )}
      </div>
    </>
  );
}

/** One file waiting to be queued: inspect, correct, choose lyrics and preset, submit. */
function ImportItem({ path }: { path: string }) {
  const studioOnline = useApp((s) => s.info?.capabilities.studio.available ?? false);
  const [insp, setInsp] = useState<InspectResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [title, setTitle] = useState("");
  const [artist, setArtist] = useState("");
  const [album, setAlbum] = useState("");
  const [cover, setCover] = useState<string | null | undefined>(undefined);
  const [mode, setMode] = useState<StudioLyricsMode>("auto");
  const [text, setText] = useState("");
  const [lrc, setLrc] = useState("");
  // Where the text / LRC came from (STUDIO_PROTOCOL 7 `lyrics.source`): "online:<service>"
  // after "Use timed lyrics" / "Use text" on a lookup result; null means the Studio's
  // default (`user_text` for pasted text, `lrc` for a chosen file). Choosing another mode
  // or another file resets it, so a song is never labelled with a source it did not use.
  const [lyricsSource, setLyricsSource] = useState<string | null>(null);
  const [preset, setPreset] = useState<StudioPreset>("full");
  const [language, setLanguage] = useState("en");
  const [lookup, setLookup] = useState<LookupResult[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const name = path.split(/[\\/]/).pop() ?? path;

  useEffect(() => {
    let cancelled = false;
    getApiSync()
      .studioInspect(path)
      .then((r) => {
        if (cancelled) return;
        setInsp(r);
        setTitle(r.title ?? name.replace(/\.[^.]+$/, ""));
        setArtist(r.artist ?? "");
        setAlbum(r.album ?? "");
      })
      .catch((e) => !cancelled && setErr(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [path]);

  const doLookup = async () => {
    setBusy("Looking up lyrics online…");
    try {
      const r = await getApiSync().studioLyricsLookup({ artist, title, album: album || null, duration_s: insp?.duration_seconds ?? null });
      setLookup(r.results);
    } catch (e) {
      toast(errorMessage(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const submit = async () => {
    setBusy("Adding to the queue…");
    try {
      await getApiSync().studioSubmit({
        input: path,
        preset,
        language,
        metadata: { title: title.trim(), artist: artist.trim(), album: album.trim() || null, ...(cover !== undefined ? { cover_path: cover } : {}) },
        lyrics: {
          mode,
          text: mode === "text" ? text : null,
          lrc: mode === "lrc" ? lrc : null,
          source: mode === "text" || mode === "lrc" ? lyricsSource : null,
        },
      });
      removeFromStudio(path);
      void refreshStudio();
    } catch (e) {
      toast(`${name}: ${errorMessage(e)}`, "error");
    } finally {
      setBusy(null);
    }
  };

  if (err) {
    return (
      <li class="studio__item studio__item--failed">
        <strong>{name}</strong>
        <Note kind="error">{err}</Note>
        <button type="button" class="button button--small" onClick={() => removeFromStudio(path)}>
          Remove
        </button>
      </li>
    );
  }
  if (!insp) {
    return (
      <li class="studio__item" aria-busy="true">
        <strong>{name}</strong> <span class="hint">Reading the file…</span>
      </li>
    );
  }
  const id = (k: string) => `studio-${k}-${path.length}-${name.length}`;
  return (
    <li class="studio__item">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <p class="studio__file">
          <strong>{name}</strong>
          <span class="hint">
            {" "}
            {[insp.container, insp.codec].filter(Boolean).join(" / ")}
            {insp.sample_rate ? `, ${formatRate(insp.sample_rate)}` : ""}
            {insp.channels ? `, ${insp.channels} ch` : ""}
            {insp.bits_per_sample ? `, ${insp.bits_per_sample}-bit` : ""}
            {insp.lossy ? ", lossy source (decoded once, never re-encoded)" : ", lossless"}
            {insp.has_video ? ", video (sound only is used)" : ""}
            {insp.duration_seconds ? `, ${formatTime(insp.duration_seconds)}` : ""}
          </span>
        </p>
        <div class="studio__fields">
          <label for={id("title")}>Title</label>
          <input id={id("title")} value={title} required onInput={(e) => setTitle((e.currentTarget as HTMLInputElement).value)} />
          <label for={id("artist")}>Artist</label>
          <input id={id("artist")} value={artist} onInput={(e) => setArtist((e.currentTarget as HTMLInputElement).value)} />
          <label for={id("album")}>Album</label>
          <input id={id("album")} value={album} onInput={(e) => setAlbum((e.currentTarget as HTMLInputElement).value)} />
          <CoverField id={id("cover")} fromFile={insp.cover_path} choice={cover} onChoice={setCover} />
        </div>

        <fieldset class="studio__lyrics">
          <legend>Lyrics</legend>
          {(
            [
              ["auto", insp.embedded_lyrics?.present ? "Automatic: use the lyrics inside the file" : "Automatic: write down what is sung (marked as unchecked)"],
              ["text", "Paste the lyrics"],
              ["lrc", "Choose an .lrc file (timed lyrics)"],
              ["none", "No lyrics (instrumental)"],
            ] as [StudioLyricsMode, string][]
          ).map(([v, label]) => (
            <label key={v} class="radio">
              <input
                type="radio"
                name={id("mode")}
                value={v}
                checked={mode === v}
                onChange={() => {
                  setMode(v);
                  setLyricsSource(null);
                }}
              />{" "}
              {label}
            </label>
          ))}
          {mode === "text" && (
            <textarea aria-label="Lyrics text, one sung line per line" rows={8} value={text} placeholder="One sung line per line" onInput={(e) => setText((e.currentTarget as HTMLTextAreaElement).value)} />
          )}
          {mode === "lrc" && (
            <div>
              <input
                type="file"
                accept=".lrc,.txt,text/plain"
                aria-label="Choose an .lrc file"
                onChange={(e) => {
                  const f = (e.currentTarget as HTMLInputElement).files?.[0];
                  if (f) {
                    setLyricsSource(null);
                    void f.text().then(setLrc);
                  }
                }}
              />
              {lrc && <pre class="studio__lrc">{lrc.slice(0, 400)}{lrc.length > 400 ? "…" : ""}</pre>}
            </div>
          )}
          <div class="row">
            <button type="button" class="button button--small" disabled={!studioOnline || busy !== null || !title.trim()} onClick={() => void doLookup()}>
              Look up lyrics online
            </button>
            <span class="hint">Sends only the title, artist and length to a lyrics service, and only when you press it.</span>
          </div>
          {lookup && lookup.length === 0 && <p class="hint">Nothing found online for this title and artist.</p>}
          {lookup && lookup.length > 0 && (
            <ul class="studio__lookup">
              {lookup.map((r, k) => (
                <li key={k}>
                  <span>
                    {r.service}
                    {r.duration_seconds ? ` (${formatTime(r.duration_seconds)})` : ""}: {(r.plain ?? r.synced_lrc ?? "").split("\n")[0]}…
                  </span>
                  {r.synced_lrc && (
                    <button type="button" class="button button--small" onClick={() => { setMode("lrc"); setLrc(r.synced_lrc!); setLyricsSource(`online:${r.service}`); }}>
                      Use timed lyrics
                    </button>
                  )}
                  {r.plain && (
                    <button type="button" class="button button--small" onClick={() => { setMode("text"); setText(r.plain!); setLyricsSource(`online:${r.service}`); }}>
                      Use text
                    </button>
                  )}
                </li>
              ))}
            </ul>
          )}
        </fieldset>

        <fieldset class="studio__preset">
          <legend>Quality preset</legend>
          {PRESETS.map((p) => (
            <label key={p.value} class="radio">
              <input type="radio" name={id("preset")} value={p.value} checked={preset === p.value} onChange={() => setPreset(p.value)} /> <strong>{p.label}</strong>
              <span class="hint"> {p.hint}</span>
            </label>
          ))}
        </fieldset>
        <Select label="Language" value={language} options={LANGUAGES} onChange={setLanguage} />

        <div class="row">
          <button type="submit" class="button button--primary" disabled={busy !== null || !title.trim() || (mode === "text" && !text.trim()) || (mode === "lrc" && !lrc.trim())}>
            Add to queue
          </button>
          <button type="button" class="button" onClick={() => removeFromStudio(path)}>
            Remove
          </button>
          {busy && <span role="status">{busy}</span>}
        </div>
      </form>
    </li>
  );
}

function Job({ job }: { job: StudioJob & { stage_progress?: number } }) {
  const idx = job.stage ? STUDIO_STAGES.indexOf(job.stage as (typeof STUDIO_STAGES)[number]) : job.state === "done" ? STUDIO_STAGES.length : -1;
  return (
    <li class={`studio__job studio__job--${job.state}`}>
      <p>
        <strong>{job.input_name}</strong>{" "}
        <span class="studio__job-state">
          {{ queued: "Waiting", running: "Working", done: "Done", failed: "Failed", cancelled: "Cancelled" }[job.state]}
        </span>
      </p>
      <ol class="studio__stages" aria-label="Stages">
        {STUDIO_STAGES.map((s, k) => (
          <li key={s} class={`studio__stage${k < idx || job.state === "done" ? " studio__stage--done" : ""}${k === idx && job.state === "running" ? " studio__stage--current" : ""}`}>
            {STAGE_LABELS[s]}
            {k === idx && job.state === "running" && job.stage_progress !== undefined ? ` ${Math.round(job.stage_progress * 100)} %` : ""}
          </li>
        ))}
      </ol>
      {(job.state === "running" || job.state === "queued") && <Progress value={job.overall} label={`${job.input_name}: overall progress`} />}
      {job.message && job.state === "running" && <p class="hint">{job.message}</p>}
      {job.state === "failed" && job.error && <Note kind="error">{job.error}</Note>}
      <div class="row">
        {(job.state === "queued" || job.state === "running") && (
          <button type="button" class="button button--small" onClick={() => void getApiSync().studioCancel(job.job_id).catch((e) => toast(errorMessage(e), "error"))}>
            Cancel
          </button>
        )}
        {job.state === "done" && job.output_song_id && (
          <button type="button" class="button button--small" onClick={() => void loadSong(job.output_song_id!)}>
            Sing it now
          </button>
        )}
      </div>
    </li>
  );
}

export function Studio() {
  const info = useApp((s) => s.info);
  const studio = useApp((s) => s.studio);
  const inbox = useApp((s) => s.studioInbox);
  const [over, setOver] = useState(false);
  if (!info) return null;
  const available = info.capabilities.studio.available && (studio?.available ?? true);
  if (!available) return <Unavailable reasons={[...new Set([...info.capabilities.studio.reasons, ...(studio?.reasons ?? [])])]} />;
  const api = getApiSync();

  const pick = async () => {
    try {
      addToStudio(await api.pickFiles());
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  };

  return (
    <div class="studio">
      <Section title="Studio: make a karaoke song">
        <Models />
        <div
          class={`studio__drop${over ? " studio__drop--over" : ""}`}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setOver(false);
            const files = Array.from(e.dataTransfer?.files ?? []).map((f) => f.name);
            if (files.length && api.htmlDropGivesPaths && api.acceptBrowserFiles) addToStudio(api.acceptBrowserFiles(files));
          }}
        >
          <p>
            <strong>Drop audio or video files here</strong>
            <br />
            mp3, m4a, flac, wav, ogg, opus, aiff, wma, mp4, mkv, mov, webm, avi. Several at once is fine.
          </p>
          <button type="button" class="button button--primary" onClick={() => void pick()}>
            Choose files…
          </button>
          {info.capabilities.studio.gpu && <p class="hint">Runs on {info.capabilities.studio.gpu}{info.capabilities.studio.vram_mb ? ` (${Math.round(info.capabilities.studio.vram_mb / 1024)} GB)` : ""}.</p>}
        </div>
        {inbox.length > 0 && (
          <ul class="studio__inbox" aria-label="Files to prepare">
            {inbox.map((p) => (
              <ImportItem key={p} path={p} />
            ))}
          </ul>
        )}
      </Section>
      <Section title="Queue" class="studio__queue">
        {(studio?.queue.length ?? 0) === 0 ? (
          <p>Nothing queued. Songs are made one at a time; you can leave them running.</p>
        ) : (
          <ul class="studio__jobs">
            {studio!.queue.map((j) => (
              <Job key={j.job_id} job={j} />
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
