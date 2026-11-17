// SPDX-License-Identifier: GPL-3.0-or-later
// Proves the built CSS only uses features the oldest supported web view has, or
// gives them a fallback (WORK_ORDER 4.10: "check each newer CSS feature against the
// baseline and give it a fallback").
//
// Baseline: Safari 16.4 (system WebKit on macOS 13) and Chrome 120 (WebView2).
// Every CSS file in dist/ is parsed with PostCSS; each rule, declaration and at-rule
// is matched against the feature table below. Support comes from caniuse data
// (the `caniuse-lite` package) where caniuse tracks the feature, otherwise from the
// first Safari/Chrome version noted in the table (from MDN browser-compat-data).
//
// A feature that is not fully supported in the baseline passes only with a fallback:
//   - it sits inside an @supports block, or
//   - the same rule declares the same property earlier with a value that does not
//     use the feature (the classic `height: 100vh; height: 100dvh;` pattern), or
//   - caniuse says it needs a prefix ("y x") and the prefixed declaration is present;
//   - or the table marks it `harmless`: a pure enhancement whose absence changes nothing
//     that matters (each such entry says why).
// Prints every feature used with its baseline support; exits 1 on a problem.
//
// Usage: node scripts/check-dist-css.mjs [distDir]
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import postcss from "postcss";
import lite from "caniuse-lite";

const here = fileURLToPath(new URL(".", import.meta.url));
const distDir = process.argv[2] ?? join(here, "..", "dist");
const BASE = { safari: "16.4", chrome: "120" };

/** Version compare "16.4" vs "15" */
const ver = (v) => v.split(".").map(Number);
const geq = (a, b) => {
  const x = ver(a);
  const y = ver(b);
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0);
    if (d) return d > 0;
  }
  return true;
};

/**
 * Feature table. `ciu` = caniuse feature id; otherwise `safari`/`chrome` = first
 * version with support (null = not supported in any released version we target).
 * Tests: prop (RegExp on property), value (RegExp on value), selector, atrule (name), params.
 */
const FEATURES = [
  { id: "custom properties", ciu: "css-variables", value: /\bvar\(/ },
  { id: "clamp()/min()/max()", ciu: "css-math-functions", value: /\b(clamp|min|max)\(/ },
  { id: "calc()", ciu: "calc", value: /\bcalc\(/ },
  { id: "CSS grid", ciu: "css-grid", prop: /^grid-template|^grid-(row|column)$/ },
  { id: "subgrid", ciu: "css-subgrid", prop: /^grid-template/, value: /\bsubgrid\b/ },
  { id: "flexbox gap", ciu: "flexbox-gap", prop: /^(gap|row-gap|column-gap)$/ },
  { id: ":focus-visible", ciu: "css-focus-visible", selector: /:focus-visible/ },
  { id: ":focus-within", ciu: "css-focus-within", selector: /:focus-within/ },
  { id: ":is()/:where()", ciu: "css-matches-pseudo", selector: /:(is|where)\(/ },
  { id: ":not() with a selector list", ciu: "css-not-sel-list", selector: /:not\([^)]*,/ },
  { id: ":has()", ciu: "css-has", selector: /:has\(/ },
  { id: "CSS nesting", ciu: "css-nesting", selector: /&/ },
  { id: "container queries", ciu: "css-container-queries", atrule: /^container$/, prop: /^container(-type|-name)?$/ },
  { id: "container query units", ciu: "css-container-query-units", value: /\d(cqw|cqh|cqi|cqb|cqmin|cqmax)\b/ },
  { id: "small/large/dynamic viewport units", ciu: "viewport-unit-variants", value: /\d(dvh|svh|lvh|dvw|svw|lvw|dvmin|dvmax|svmin|svmax|lvmin|lvmax)\b/ },
  { id: "viewport units", ciu: "viewport-units", value: /\d(vh|vw|vmin|vmax)\b/ },
  { id: "media query range syntax", ciu: "css-media-range-syntax", atrule: /^media$/, params: /[<>]=?/ },
  { id: "prefers-reduced-motion", ciu: "prefers-reduced-motion", atrule: /^media$/, params: /prefers-reduced-motion/ },
  { id: "prefers-color-scheme", ciu: "prefers-color-scheme", atrule: /^media$/, params: /prefers-color-scheme/ },
  { id: "logical properties", ciu: "css-logical-props", prop: /-(inline|block)(-start|-end)?$|^inset-(inline|block)/ },
  { id: "inset shorthand", safari: "14.1", chrome: "87", prop: /^inset$/ },
  { id: "object-fit", ciu: "object-fit", prop: /^object-fit$/ },
  { id: "::backdrop", ciu: "mdn-css-backdrop-pseudo-element", selector: /::backdrop/ },
  { id: "backdrop-filter", ciu: "css-backdrop-filter", prop: /^backdrop-filter$/ },
  { id: "filter", ciu: "css-filters", prop: /^filter$/ },
  { id: "user-select", ciu: "user-select-none", prop: /^user-select$/ },
  { id: "text-wrap", ciu: "css-text-wrap-balance", prop: /^text-wrap/ },
  { id: "cascade layers", ciu: "css-cascade-layers", atrule: /^layer$/ },
  { id: "@scope", ciu: "css-cascade-scope", atrule: /^scope$/ },
  { id: "lab()/lch()/oklab()/oklch()", ciu: "css-lch-lab", value: /\b(ok)?(lab|lch)\(/ },
  { id: "color-mix()", safari: "16.2", chrome: "111", value: /\bcolor-mix\(/ },
  { id: "light-dark()", safari: "17.5", chrome: "123", value: /\blight-dark\(/ },
  { id: "relative colour syntax", safari: "18", chrome: "119", value: /\b(rgb|hsl|lab|lch|oklab|oklch)\(\s*from\b/ },
  { id: "scrollbar styling", ciu: "css-scrollbar", prop: /^scrollbar-(color|width)$/ },
  { id: "scrollbar-gutter", safari: "18.2", chrome: "94", prop: /^scrollbar-gutter$/ },
  { id: "overscroll-behavior", ciu: "css-overscroll-behavior", prop: /^overscroll-behavior/ },
  // Only matters on touch screens; desktop Safari has none and ignores it (drag still works with a mouse).
  { id: "touch-action", ciu: "css-touch-action", prop: /^touch-action$/, harmless: "only affects touch screens; ignored where unsupported" },
  { id: "font-variant-numeric", ciu: "font-variant-numeric", prop: /^font-variant-numeric$/ },
  { id: "appearance", ciu: "css-appearance", prop: /^appearance$/ },
  { id: "scroll-behavior", ciu: "css-scroll-behavior", prop: /^scroll-behavior$/ },
  { id: "position: sticky", ciu: "css-sticky", prop: /^position$/, value: /\bsticky\b/ },
  { id: "display: contents", ciu: "css-display-contents", prop: /^display$/, value: /\bcontents\b/ },
  { id: "accent-color", safari: "15.4", chrome: "93", prop: /^accent-color$/ },
  { id: "aspect-ratio", safari: "15", chrome: "88", prop: /^aspect-ratio$/ },
  { id: "color-scheme", safari: "13", chrome: "81", prop: /^color-scheme$/ },
  { id: "place-items/place-content", safari: "11", chrome: "59", prop: /^place-(items|content|self)$/ },
  { id: "text-decoration-thickness", safari: "12.1", chrome: "89", prop: /^text-decoration-thickness$/ },
  { id: "overflow: clip", safari: "16", chrome: "90", prop: /^overflow(-x|-y)?$/, value: /\bclip\b/ },
  { id: "translate/rotate/scale properties", safari: "14.1", chrome: "104", prop: /^(translate|rotate|scale)$/ },
  { id: "@property", safari: "16.4", chrome: "85", atrule: /^property$/ },
  { id: "@starting-style", safari: "17.5", chrome: "117", atrule: /^starting-style$/ },
  { id: ":user-valid/:user-invalid", safari: "16.5", chrome: "119", selector: /:user-(valid|invalid)/ },
  { id: "content-visibility", safari: "18", chrome: "85", prop: /^content-visibility$/ },
  { id: "field-sizing", safari: null, chrome: "123", prop: /^field-sizing$/ },
  { id: "anchor positioning", safari: "26", chrome: "125", prop: /^(anchor-name|position-anchor|position-area|position-try)/ },
  { id: "view transitions", ciu: "view-transitions", prop: /^view-transition/ },
  { id: "font-size-adjust", safari: "17", chrome: "127", prop: /^font-size-adjust$/ },
  { id: "rlh unit", safari: "18", chrome: "111", value: /\d(rlh)\b/ },
  { id: "round()/mod()/rem()", safari: "15.4", chrome: "125", value: /\b(round|mod|rem)\(/ },
  { id: "CSS transitions", ciu: "css-transitions", prop: /^transition/ },
  { id: "CSS animation", ciu: "css-animation", prop: /^animation/ },
];

const features = new Map();
function support(f, browser) {
  if (f.ciu) {
    let data = features.get(f.ciu);
    if (!data) {
      const mod = JSON.parse(JSON.stringify(lite.feature(lite.features[f.ciu])));
      features.set(f.ciu, (data = mod));
    }
    const stats = data.stats[browser] ?? {};
    // Exact version, or the nearest older one caniuse lists.
    const versions = Object.keys(stats).filter((v) => !v.includes("-") && /^[\d.]+$/.test(v) && geq(BASE[browser], v));
    versions.sort((a, b) => (geq(a, b) ? -1 : 1));
    const s = stats[BASE[browser]] ?? stats[versions[0]] ?? "u";
    return s;
  }
  const first = f[browser];
  if (first === null || first === undefined) return "n";
  return geq(BASE[browser], first) ? "y" : "n";
}
const ok = (s) => /^y\b/.test(s) && !/\bx\b/.test(s);
const needsPrefix = (s) => /\bx\b/.test(s) && /^[ya]/.test(s);

function* walkFiles(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) yield* walkFiles(p);
    else yield p;
  }
}

/**
 * Checks one stylesheet. Adds to `used` (feature id -> usage) and returns problems.
 * Exported for the checker's own test (src/csscheck.test.ts).
 */
export function checkCssText(css, name, used = new Map()) {
  const problems = [];
  const rel = name;
  const root = postcss.parse(css, { from: name });

  const insideSupports = (node) => {
    for (let p = node.parent; p; p = p.parent) if (p.type === "atrule" && p.name === "supports") return true;
    return false;
  };
  const record = (f, node, where, fallback) => {
    let u = used.get(f.id);
    if (!u) used.set(f.id, (u = { f, count: 0, safari: support(f, "safari"), chrome: support(f, "chrome") }));
    u.count++;
    for (const b of ["safari", "chrome"]) {
      const s = u[b];
      if (ok(s)) continue;
      if (insideSupports(node) || fallback(s) || f.harmless) continue;
      problems.push(`${rel}: ${f.id} is "${s}" in ${b} ${BASE[b]} and has no fallback (${where})`);
    }
  };

  root.walk((node) => {
    if (node.type === "rule") {
      if (node.parent && node.parent.type === "rule") {
        const f = FEATURES.find((x) => x.id === "CSS nesting");
        record(f, node, `nested rule "${node.selector}"`, () => false);
      }
      for (const f of FEATURES) if (f.selector && f.selector.test(node.selector)) record(f, node, `selector "${node.selector.slice(0, 80)}"`, () => false);
    } else if (node.type === "atrule") {
      for (const f of FEATURES) {
        if (!f.atrule || !f.atrule.test(node.name)) continue;
        if (f.params && !f.params.test(node.params)) continue;
        record(f, node, `@${node.name} ${node.params}`.slice(0, 90), () => false);
      }
    } else if (node.type === "decl") {
      const prop = node.prop.toLowerCase();
      const bare = prop.replace(/^-(webkit|moz|ms)-/, "");
      for (const f of FEATURES) {
        if (f.prop && !f.prop.test(bare)) continue;
        if (f.value && !f.value.test(node.value)) continue;
        if (!f.prop && !f.value) continue;
        if (bare !== prop) continue; // prefixed forms are the fallbacks themselves
        const siblings = node.parent?.nodes?.filter((n) => n.type === "decl") ?? [];
        const idx = siblings.indexOf(node);
        const fallback = (s) => {
          // Prefixed declaration present in the same rule?
          if (needsPrefix(s) && siblings.some((n) => n.prop.toLowerCase() === `-webkit-${bare}`)) return true;
          // Earlier declaration of the same property whose value does not use the feature?
          if (!f.value) return false;
          return siblings.slice(0, idx).some((n) => n.prop.toLowerCase() === prop && !f.value.test(n.value));
        };
        record(f, node, `${node.prop}: ${node.value}`.slice(0, 90), fallback);
      }
    }
  });
  return problems;
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);
if (isMain) {
  const used = new Map();
  const problems = [];
  let files = 0;
  for (const file of walkFiles(distDir)) {
    if (!file.endsWith(".css")) continue;
    files++;
    problems.push(...checkCssText(readFileSync(file, "utf8"), relative(distDir, file), used));
  }

  if (files === 0) {
    console.error(`FAIL no .css files under ${distDir} (run vite build first)`);
    process.exit(1);
  }
  const rows = [...used.values()].sort((a, b) => a.f.id.localeCompare(b.f.id));
  console.log(`CSS features used in ${files} file(s), support in Safari ${BASE.safari} / Chrome ${BASE.chrome} (${"caniuse-lite"} data or MDN versions):`);
  for (const u of rows) {
    console.log(`  ${u.f.id.padEnd(36)} x${String(u.count).padEnd(4)} safari ${u.safari.padEnd(6)} chrome ${u.chrome.padEnd(6)} ${u.f.ciu ? `[caniuse ${u.f.ciu}]` : `[since Safari ${u.f.safari ?? "never"}, Chrome ${u.f.chrome ?? "never"}]`}${u.f.harmless && !(ok(u.safari) && ok(u.chrome)) ? ` (allowed: ${u.f.harmless})` : ""}`);
  }
  if (problems.length) {
    console.error(`\n${problems.length} problem(s):\n${problems.join("\n")}`);
    process.exit(1);
  }
  console.log(`OK: every CSS feature in dist is supported by Safari ${BASE.safari} and Chrome ${BASE.chrome}, or has a fallback.`);
}
