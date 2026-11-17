// SPDX-License-Identifier: GPL-3.0-or-later
/// <reference types="vitest/config" />
// Vite build for the Jukeoke web UI.
//
// Browser baseline (docs/ARCHITECTURE.md section 3 and 6):
//   - macOS 13.6.9 (the owner's MacBook Air) renders the UI with the system WebKit,
//     treated as Safari 16.4;
//   - Windows renders it with WebView2 (Chromium), treated as Chrome 120.
// Everything Vite emits (JS and CSS) is lowered to that baseline, and
// scripts/check-dist-syntax.mjs re-parses the output afterwards to prove it.
import { readFileSync } from "node:fs";
import { defineConfig, type Plugin } from "vite";
import preact from "@preact/preset-vite";

/** Targets understood by Vite / Oxc / Lightning CSS. Keep in sync with check-dist-syntax.mjs. */
export const BROWSER_TARGETS = ["safari16.4", "chrome120"];

// branding.json (repo root) is the single source of truth for the product name
// (docs/ARCHITECTURE.md section 9). The page title is injected from it, so
// index.html does not hard-code the name.
const branding = JSON.parse(
  readFileSync(new URL("../../branding.json", import.meta.url), "utf8"),
) as { name: string };

function brandingTitle(): Plugin {
  return {
    name: "jukeoke-branding-title",
    transformIndexHtml: (html) => html.replaceAll("%APP_NAME%", branding.name),
  };
}

export default defineConfig({
  plugins: [preact(), brandingTitle()],
  // Relative asset URLs: the bundle is served by Tauri's custom protocol, never from
  // a web server root, and must not depend on where it is mounted.
  base: "./",
  clearScreen: false,
  server: {
    // `tauri dev` points the window at this URL (tauri.conf.json build.devUrl).
    port: 5173,
    strictPort: true,
  },
  build: {
    target: BROWSER_TARGETS,
    cssTarget: BROWSER_TARGETS,
    outDir: "dist",
    emptyOutDir: true,
    // No source maps in the shipped app: smaller, and nothing to leak.
    sourcemap: false,
    // Everything is local; inlining tiny assets as data: URIs is fine with our CSP.
    assetsInlineLimit: 4096,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
