// SPDX-License-Identifier: GPL-3.0-or-later
// Checks one Settings/smoke-test.json written by `Jukeoke.exe --smoke-test` in a
// portable layout, and prints the measured numbers.
//
// Usage: node verify-smoke-report.mjs <report.json> <expected portable root> <exit code>
// Exit code 0 when every check passes.
import { readFileSync } from "node:fs";

const [reportPath, expectedRoot, exitCode] = process.argv.slice(2);
const report = JSON.parse(readFileSync(reportPath, "utf8"));

/** Windows paths compare case-insensitively, with either slash, without a trailing one. */
const norm = (p) => String(p ?? "").replaceAll("/", "\\").replace(/\\+$/, "").toLowerCase();
const join = (...parts) => parts.map((p) => String(p).replace(/[\\/]+$/, "")).join("\\");

const root = norm(expectedRoot);
const checks = [
  ["app exit code is 0", exitCode === "0", `exit code ${exitCode}`],
  ["report says ok", report.ok === true, report.failure ?? JSON.stringify(report.checks)],
  ["portable mode", report.layout?.mode === "portable", report.layout?.reason],
  ["portable root is the expected one", norm(report.layout?.portable_root) === root, report.layout?.portable_root],
  ["exe runs from that root", norm(report.app?.exe).startsWith(root + "\\"), report.app?.exe],
  [
    "WebView2 profile is <root>\\Settings\\webview-windows",
    norm(report.webview?.user_data_dir) === norm(join(expectedRoot, "Settings", "webview-windows")),
    report.webview?.user_data_dir,
  ],
  [
    "WebView2 profile was created there",
    (report.webview?.user_data_dir_entries ?? []).includes("EBWebView"),
    JSON.stringify(report.webview?.user_data_dir_entries),
  ],
  ["UI rendered the app name", report.ui?.renderedTitle === report.app?.name, report.ui?.renderedTitle],
];

let failed = 0;
for (const [name, pass, detail] of checks) {
  if (!pass) failed += 1;
  console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? `  (${detail})` : ""}`);
}

const marks = Object.fromEntries((report.timings_ms_since_main ?? []).map((m) => [m.name, Math.round(m.ms)]));
const ui = report.ui ?? {};
console.log(`  webview: ${report.webview?.version}  loader: ${report.webview?.loader?.route}  (${report.webview?.loader?.runtime_dll ?? "-"})`);
console.log(`  rust timings since main (ms): ${JSON.stringify(marks)}  total ${Math.round(report.total_ms_since_main)}`);
console.log(
  `  ui timings since page start (ms): script ${ui.scriptStartMs?.toFixed(1)}, first paint ${ui.firstPaintMs?.toFixed(1)}, ` +
    `first contentful paint ${ui.firstContentfulPaintMs?.toFixed(1)}, app_info round trip ${ui.appInfoRoundTripMs?.toFixed(1)}, report sent ${ui.reportSentMs?.toFixed(1)}`,
);
console.log(`  viewport: ${JSON.stringify(ui.viewport)}  UA: ${ui.userAgent}`);
process.exit(failed === 0 ? 0 : 1);
