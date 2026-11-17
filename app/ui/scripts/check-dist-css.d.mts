// SPDX-License-Identifier: GPL-3.0-or-later
// Types for the parts of check-dist-css.mjs the tests use.
export function checkCssText(css: string, name: string, used?: Map<string, { count: number; safari: string; chrome: string }>): string[];
