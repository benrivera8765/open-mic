// SPDX-License-Identifier: GPL-3.0-or-later
// Typed wrappers around the Rust commands in app/src-tauri/src/commands.rs.
// The field names match the Rust structs (serde renames them to camelCase).
// The UI never touches audio; it only calls commands and listens to events.
import { invoke, isTauri } from "@tauri-apps/api/core";

/** How the app decided where its data lives (app/src-tauri/src/portable.rs). */
export type DataMode = "portable" | "dev" | "user";

export interface AppInfo {
  /** Product name, read at runtime from Tauri's package info (tauri.conf.json productName). */
  name: string;
  /** App version (Cargo package version). */
  version: string;
  /** Bundle identifier (tauri.conf.json identifier). */
  identifier: string;
  mode: DataMode;
  /** The detected portable root (<root> holding Windows/, Mac/, Library/ ...), or null. */
  portableRoot: string | null;
  /** Folder that holds Settings/, Library/, Recordings/ and Models/ in this mode. */
  dataRoot: string;
  /** Where the web view keeps its profile; null when the web view is non-persistent (macOS). */
  webviewDataDir: string | null;
  /** Why the mode was chosen, in plain words (shown to the user). */
  modeReason: string;
  /** True when macOS runs the app from a randomised App Translocation path. */
  translocated: boolean;
  os: string;
  arch: string;
  /** Version of the system web view (WebView2 on Windows, WebKit on macOS) or an error text. */
  webviewVersion: string;
  /** True when started with --smoke-test. */
  smokeTest: boolean;
}

export interface PingReply {
  /** Echo of the text the UI sent. */
  echo: string;
  /** Milliseconds since the Rust process entered main(). */
  uptimeMs: number;
}

/** What the UI reports back in --smoke-test mode (merged into Settings/smoke-test.json). */
export interface SmokeReport {
  /** performance.now() values, in ms since the page's time origin. */
  scriptStartMs: number;
  firstPaintMs: number | null;
  firstContentfulPaintMs: number | null;
  reportSentMs: number;
  /** Round trip of the first app_info command, measured in JS. */
  appInfoRoundTripMs: number;
  /** Text the page actually rendered, read back from the DOM. */
  renderedTitle: string;
  userAgent: string;
  viewport: { width: number; height: number; devicePixelRatio: number };
}

export const inTauri = (): boolean => isTauri();

export const getAppInfo = (): Promise<AppInfo> => invoke<AppInfo>("app_info");

export const ping = (text: string): Promise<PingReply> => invoke<PingReply>("ping", { text });

export const sendSmokeReport = (report: SmokeReport): Promise<void> =>
  invoke<void>("smoke_report", { report });
