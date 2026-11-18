# Visual design brief: the jukebox skin

Source: the owner's four reference photos (kept outside the repository, in
`../reference-images/`, never committed or shipped) and WORK_ORDER section 4.7.
We take the **design language** of a classic bubbler jukebox, not any brand: no
manufacturer name, no logo, no winged badge shape, no copied artwork. Everything is
drawn in code (SVG, CSS, canvas).

## What the photos show (and what we take from each)

1. **The glow (photo 1, dark room).** The jukebox is the light source. A thick
   translucent arch glows hot pink to red across the top; the side pilasters fade
   green -> yellow -> orange top to bottom; a lit lower "skirt" panel glows lime,
   white and pink around a dark diamond-lattice grille with a compass-star medallion
   in a red disc. Light looks like it sits *inside* frosted plastic: bright core,
   soft falloff, a hard specular line along each tube.
   -> Our arch and columns are frosted-plastic light tubes with a bright inner core and
   a soft bloom outside, on a dark surround.
2. **The materials (photo 2).** Walnut sides with straight grain; a lighter burl
   front panel; polished, ribbed chrome bands (three stacked rings with red enamel
   between them) where the column meets the arch; a chrome-framed title-strip window
   with a curved glass hood; small chrome-bezel plates with black glass and white
   condensed caps ("MAKE SELECTIONS").
   -> Buttons are chrome-bezel plates with black glass faces and white condensed
   uppercase labels; red enamel accents; walnut/burl as the cabinet fill.
3. **The silhouette (photo 3).** Tall rounded arch; a column each side capped by a
   carved scroll and chrome rings; centre title-strip window; two selection plates
   either side of a stained-glass art panel; a lower grille with scrollwork and a
   compass-star medallion; round feet.
   -> Proportions: arch about 1/6 of the height, columns about 1/12 of the width each,
   console about 1/5 of the height.
4. **Screen in the cabinet (photo 4).** The closest layout: a screen is the
   centrepiece, flanked by two vertical gradient light strips, framed by the arch and
   columns; chrome rings on the columns; a red diamond-lattice grille with a gold/
   chrome medallion below.
   -> The app's content (library, lyrics stage, mixer) sits in the "screen". The
   arch, columns and console frame it.

## Window anatomy

```
 _______________________________________________________________
/  ARCH CREST: glowing tube + bubble tube + chrome crest ornament \
| [C]  ______________________________________________________  [C]|
| [O] |                                                      | [O]|
| [L] |                 SCREEN (app content)                 | [L]|
| [U] |   library = title-strip cards / stage = lyrics ticker | [U]|
| [M] |______________________________________________________| [M]|
| [N]  CONSOLE: [plate] [<<][PLAY][>>]  (( KEY DIAL ))  [plate] [N]|
|_____  grille + medallion  ____  mixer faders as plates  ________|
```

- **Arch crest**: an SVG path the full window width, height clamp(56px, 9vh, 120px).
  Outer chrome rim, glowing tube, inner chrome rim. A centred crest ornament of our
  own design (a stylised music note inside a scroll), plus the app badge (section below).
- **Columns**: left and right, width clamp(28px, 4vw, 72px). Glowing gradient tube
  with a bubble tube running inside it; three chrome rings with red enamel at the top.
- **Console**: height clamp(96px, 16vh, 180px). Transport buttons as chrome-bezel
  plates; the **key dial** in the centre, the largest and most satisfying control: a
  chrome knurled ring around a black glass face showing "A major -> B major (+2)",
  with up/down plates and detents (one per semitone), a click-free animated pointer,
  keyboard `[` / `]` and arrow keys, scroll wheel. A small diamond-lattice grille and
  the compass-star medallion sit beneath or beside it.
- **Screen**: the app content, with a subtle curved-glass sheen at the top edge and
  two vertical light strips at its sides (photo 4).

## Light and colour

Palette (the rotating colour cylinders): pink `#ff3d8b`, red `#ff2a2a`, orange
`#ff8a1f`, yellow `#ffd23f`, green `#4dff6a`, mint `#7dffd8`, violet `#9b6bff`.
Chrome: a vertical gradient of near-white, cool grey and a dark band with a thin
bright specular line. Red enamel `#c8102e` with a glossy highlight. Walnut
`#5a3a22` -> `#3b2414` with fine grain; burl `#b8834a` -> `#8a5a2b`. Black glass
`#0d0d10` with a faint reflection gradient. Screen background: deep aubergine
`#14101c`.

The light drifts slowly through the palette (one full cycle about 40 s), like the
rotating cylinders in a real bubbler. Implement it by translating a tall gradient
strip inside each clipped tube with a CSS `transform` animation (composited, near
zero CPU), not by repainting gradients.

## Bubbles

Small translucent bubbles (bright rim, transparent middle, tiny highlight) rising
inside narrow tubes that follow the frame: up each column and over the arch to the
crest, like real bubble tubes. Bubbles vary in size and speed, occasionally cluster,
and speed up slightly near the top. Rules: one canvas layer (or CSS motion along SVG
paths if Safari 16.4 supports what is needed), capped at 30 fps, at most ~60 bubbles,
paused when the window is hidden or minimised, off under `prefers-reduced-motion`,
and a settings switch. They must never affect audio (audio is native and unaffected
by the web view, but keep the main thread light so the ticker stays smooth).

## Library: title-strip cards

Each song is a jukebox title strip: a cream card `#f6efd9` with a thin red border
line top and bottom, the title in a bold condensed face, the artist below in a
lighter weight, a small cover thumbnail at the left, the key ("A maj") and length
on the right, a selection code like "A7" in a chrome tab. Cards sit in a chrome-
framed rack, two columns on wide windows, one on narrow ones. Search box styled as
a black-glass plate. Shows free space on the stick.

## Stage (singing view)

Dark screen, the lyrics ticker large and centred vertically, the marker a slim
vertical chrome needle with a soft pink glow at about one third from the left. Sung
letters change to a warm colour (default hot pink `#ff3d8b` -> with the next letters
in cream). Above the ticker, small: title, artist, key, time. Below: the next line
preview is not needed (the ticker itself shows what is coming); in long breaks a
countdown "Singing in 0:08" sits in the ticker's lane.

**Changed at the owner's request, 18 Nov 2026 (D-218): static lines with a colour wipe**
(the default; the ticker above stays as "Scrolling ticker" in Settings). Modelled on the
owner's reference frames of a karaoke video: the current line stands still, centred,
large, in a heavy face (Atkinson Hyperlegible Next 800) with a thin dark outline and a
soft shadow, legible on any background. The sung part turns warm gold (`#ffc21a`), the
rest is white (`#f4f4f4`), and the boundary sweeps smoothly through the letters, word by
word. The next line waits underneath at about 0.6x size and 60 % opacity; when the line
has been sung the page moves up in 0.2 s (the next line grows into place, the old one
rises and fades, the one after fades in below). Before a line that follows a pause, a
small gold block left of its first letter, as tall as the capitals and dark at its left
end, is eaten from the left and is gone the moment the first word starts (the reference's
lead-in bar). Long rests: the screen clears to the "Singing in 0:08" countdown in the
same lettering, and the line comes back 3 s before it is sung. Centred rather than
left-aligned like the reference crops: the jukebox screen is a symmetric arch, short
lines would leave its right half empty, and a centred line sits under the badge; the
lead-in bar moves with the line's first letter.

**Added at the owner's request, 18 Nov 2026 (D-219): the progress needle on the wipe.**
The scrolling ticker's marker needle (a slim solid shaft with a small triangular head, in
the marker colour, default mint `#7dffd8`) comes back in the static display, riding the
edge of the gold: it stands exactly on the gold/white boundary inside the letters, a
little taller than the row of text (0.12 em above and below), with the letters' soft
shadow so it reads on white. Before a line it waits at the first letter (beside the
lead-in block); on a two-row line it moves to the second row when the gold does; at a line
change it moves up with the next line; it is gone while the countdown is shown. Settings >
Lyrics > "Show the progress bar" turns it off; "Marker" sets its colour.

## Badge

An original badge for "Jukeoke": a rounded shield or lozenge (not a winged shape),
chrome rim, red enamel field, the name in a cream script or condensed italic, a tiny
star. Drawn in SVG from `branding.json`'s name so renaming updates it.

## Typography

Bundled, openly licensed fonts only (SIL OFL), included in the app, never loaded
from the network. Suggested: a condensed display face for labels and cards, and a
highly legible sans for the lyrics (large x-height, clear letter shapes, ligatures
off). Record the fonts and licences in THIRD_PARTY_LICENSES.md.

## Scale and performance

- Design for 1440x900 points first, check 1280x800, and scale cleanly to 4K and full
  screen. Frame sizes use `clamp()`; nothing is a fixed pixel bitmap.
- Idle CPU/GPU low: no animation when hidden, frame rate capped, glow by compositor
  transforms, no large animated blur filters.
- A plain, functional mode (skin off) stays available and is what tests run against.

## As built (Stage 1, 2026-11-17)

Where the skin (`app/ui/src/skin/`, wired in by `App.tsx` and
`components/Jukebox.tsx`) differs from the brief above. Reasons are in DECISIONS
D-102 to D-111.

- **Frame sizes** scale proportionally to 4K instead of stopping at 72/120/180 px:
  columns clamp(28px, 4vw, 154px), arch clamp(56px, 9vh, 194px), console
  clamp(104px, 17.5vh, 378px), screen corner radius clamp(24px, min(11vw, 17.5vh),
  400px), in container units with a vw/vh fallback. The key dial is 0.66 x the console
  height (about 104 px at 1440x900).
- **Arch geometry**: one tube centre line (column, a corner arc concentric with the
  screen's rounded corner, then a large tangent arc to the crest); rims, glow, bubble
  tube and masks are offsets of it.
- **Crest**: the app badge sits on the arch apex, held by two chrome clamps with red
  enamel rings (the column motif). The stylised-note crest ornament is only a fallback
  when no badge is passed; scroll brackets were dropped because beside a badge they
  could read as wings.
- **Badge**: an elongated, round-cornered hexagonal lozenge with chrome rim, red
  enamel field, cream pinstripe, the name in Pacifico with a swash underline, and one
  four-point star.
- **Light drift**: a Web Animations transform on gradient strips inside masked regions,
  one palette cycle per 40 s, advanced in steps 12 times a second (not every frame); it
  flows from the crest outward and down the columns. The two light strips inside the
  screen are **static**: nothing but the ticker moves inside the screen.
- **Materials**: static art is pre-drawn SVG shown as images, and the cabinet is a
  procedurally generated walnut tile; the burl front was dropped (it read as marble).
- **Bubbles**: one full-window canvas, at most 30 fps and 60 bubbles, prewarmed so the
  tubes start full, clusters of 3-5. They also stop while the native window is
  minimised, not only when the page is hidden.
- **Console**: transport plates (restart, back 5 s, play/pause lit while playing,
  forward 5 s) over a seek bar and time; the key dial in the centre; vertical Guide,
  Backing and Master faders and a clip lamp at the right; grille and medallion wings.
  Backing on/off, drums, band level, output device and export stay in the Mixer
  panel. Navigation is a row of plates along the top of the screen.
- **Key dial**: 270-degree sweep, one detent per semitone, -/+ plates, drag, wheel and
  keys (arrows, `[` `]`, `+` `-`, PageUp/PageDown = 2, Home/End, `0` or double-click =
  original key). It commits 160 ms after the hand stops (each key change rebuilds the
  pitch shifter); the app-wide key shortcuts act at once.
- **Library**: title-strip cards in a chrome rack, 2 columns on a laptop, more on 4K,
  1 when narrow; a drawn record stands in for missing cover art.
- **Deleting songs** (the owner's request of 18 Nov 2026, D-230): a small black-glass
  plate with a drawn bin on the hovered, focused or current strip (its glass lights red
  under the pointer); "Select" lights a selector lamp on each strip's chrome tab; the
  question before a delete is a chrome-framed black-glass panel with plates, the
  confirming plate lit red; "Deleted songs" shows the deleted strips faded to grey
  paper with grey rules and a chrome tab holding a bin, with Restore and
  "Delete for good" plates.
- **Stage marker**: a slim solid needle with a small triangular head in the marker
  colour, default mint `#7dffd8` (not a chrome needle with a pink glow), so it stands out
  against the sung letters: fixed in the scrolling ticker, on the wipe edge in the static
  display (D-219, same drawing, sized with the text). Until D-218 the sung letters were
  pink `#ff3d8b` and the upcoming ones cream `#f6efd9`; both displays now default to
  gold and white.
- **Fonts**: Oswald (display: plates, labels, title strips), Atkinson Hyperlegible Next
  (lyrics, weight 700 in the ticker, ligatures and contextual alternates off) and
  Pacifico (badge only), all SIL OFL 1.1, bundled unmodified.
- **Transport icons** are drawn SVG, not Unicode media symbols (WebKit showed those as
  colour emoji).
- **Full screen** keeps the frame and shows the stage alone in the screen.
- Not yet measured in Safari/WKWebView on a Mac: drift, bubbles and their CPU cost
  need the owner's MacBook (compare with motion "reduce").
