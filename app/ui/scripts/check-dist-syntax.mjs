// SPDX-License-Identifier: GPL-3.0-or-later
// Proves the built UI only uses JavaScript the oldest supported web view can run.
//
// Baseline: Safari 16.4 (system WebKit on macOS 13, the owner's MacBook Air) and
// Chrome 120 (WebView2 on Windows 11). Safari 16.4 supports all of ES2022 syntax
// (class static blocks and RegExp lookbehind arrived exactly in 16.4) and none of
// the syntax added after it (RegExp `v` flag, import attributes, `using`, RegExp
// modifiers, duplicate named groups). So every emitted .js file must parse as
// ECMAScript 2022; acorn also validates regular-expression syntax and flags
// against that version.
//
// Syntax is only half the story: a few newer *APIs* would parse fine but fail at
// run time. A short deny-list catches the ones a bundler or dependency is most
// likely to introduce; it is a heuristic, not a proof.
//
// Usage: node scripts/check-dist-syntax.mjs [distDir]   (exit code 1 on failure)
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "acorn";

const here = fileURLToPath(new URL(".", import.meta.url));
const distDir = process.argv[2] ?? join(here, "..", "dist");

/** APIs newer than Safari 16.4 (first Safari version that has them in brackets). */
const DENY_APIS = [
  { re: /\bPromise\.withResolvers\b/, why: "Promise.withResolvers (Safari 17.4)" },
  { re: /\b(?:Object|Map)\.groupBy\b/, why: "Object/Map.groupBy (Safari 17.4)" },
  { re: /\brequestIdleCallback\b/, why: "requestIdleCallback (not in Safari 16/17)" },
  { re: /\.(?:union|intersection|symmetricDifference|isSubsetOf|isSupersetOf|isDisjointFrom)\(/, why: "Set methods (Safari 17)" },
  { re: /\bnavigator\.gpu\b/, why: "WebGPU (not on macOS 13)" },
  { re: /\bPromise\.try\b/, why: "Promise.try (Safari 18.2)" },
  { re: /\bURL\.canParse\b/, why: "URL.canParse (Safari 17)" },
  { re: /\bAbortSignal\.any\b/, why: "AbortSignal.any (Safari 17.4)" },
  { re: /\bRegExp\.escape\b/, why: "RegExp.escape (Safari 18.2)" },
  { re: /\bstartViewTransition\b/, why: "View Transitions (Safari 18)" },
  { re: /\.(?:showPopover|hidePopover|togglePopover)\(/, why: "Popover API (Safari 17)" },
  { re: /\bscheduler\.(?:postTask|yield)\b/, why: "scheduler API (not in Safari)" },
  { re: /\.transferToFixedLength\(/, why: "ArrayBuffer transfer (Safari 17.4)" },
  { re: /\.letterSpacing\s*=|\.wordSpacing\s*=/, why: "canvas letterSpacing/wordSpacing (Safari 18.4)" },
];

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* walk(path);
    else yield path;
  }
}

let failures = 0;
let checked = 0;
for (const file of walk(distDir)) {
  if (!/\.(m?js)$/.test(file)) continue;
  checked += 1;
  const source = readFileSync(file, "utf8");
  const rel = relative(distDir, file);
  try {
    parse(source, { ecmaVersion: 2022, sourceType: "module" });
  } catch (err) {
    failures += 1;
    console.error(`FAIL ${rel}: not valid ES2022 (Safari 16.4 baseline): ${err.message}`);
    continue;
  }
  for (const { re, why } of DENY_APIS) {
    if (re.test(source)) {
      failures += 1;
      console.error(`FAIL ${rel}: uses ${why}`);
    }
  }
}

if (checked === 0) {
  console.error(`FAIL no .js files found under ${distDir} (run vite build first)`);
  process.exit(1);
}
if (failures > 0) {
  console.error(`${failures} problem(s) in ${checked} file(s).`);
  process.exit(1);
}
console.log(`OK: ${checked} JS file(s) in ${relative(process.cwd(), distDir) || distDir} parse as ES2022 (Safari 16.4 / Chrome 120 baseline).`);
