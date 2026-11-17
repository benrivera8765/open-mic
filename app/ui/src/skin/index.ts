// SPDX-License-Identifier: GPL-3.0-or-later
// The jukebox skin: a self-contained layer of Preact components that wraps the
// app's screens (docs/DESIGN.md, docs/ARCHITECTURE.md section 6). Importing this
// module loads its stylesheet and the bundled fonts. See README.md in this folder
// for how to wire it into the app.

import "./skin.css";

export { JukeboxFrame, DRIFT_FPS, LIGHT_CYCLE_MS, paletteGradient, type JukeboxFrameProps } from "./JukeboxFrame";
export { KeyDial, type KeyDialProps } from "./KeyDial";
export { PlateButton, type PlateButtonProps } from "./PlateButton";
export { Fader, type FaderProps } from "./Fader";
export {
  TitleStripCard,
  TitleStripRack,
  formatDuration,
  type TitleStripCardProps,
  type TitleStripRackProps,
} from "./TitleStripCard";
export { Badge, appName, type BadgeProps } from "./Badge";
export { Medallion, type MedallionProps } from "./Medallion";
export { Grille, type GrilleProps } from "./Grille";
export { CrestOrnament } from "./CrestOrnament";
export { TransportIcon, type TransportIconName } from "./TransportIcon";
export { Bubbles, bubbleStats, type BubblesProps } from "./Bubbles";
export {
  resolveMotion,
  useResolvedMotion,
  useSystemReducedMotion,
  usePageVisible,
  type Motion,
  type MotionSetting,
} from "./motion";
export { formatSemitones, prettyKeyLabel } from "./keydial-math";
