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
