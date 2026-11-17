// SPDX-License-Identifier: GPL-3.0-or-later
// Unique ids for SVG gradients, patterns and filters inside skin components.
//
// Several instances of the same component can be on a page (the demo shows two
// medallions and three grilles); `url(#id)` references must not collide, or a
// hidden first instance would break the others in some browsers.

import { useId } from "preact/hooks";

/** A document-unique id prefix, safe inside `url(#...)` (letters, digits, dashes only). */
export function useSvgId(prefix: string): string {
  const raw = useId();
  return `${prefix}-${raw.replace(/[^A-Za-z0-9-]/g, "")}`;
}
