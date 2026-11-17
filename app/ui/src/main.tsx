// SPDX-License-Identifier: GPL-3.0-or-later
// Entry point of the web UI. Picks the API (Tauri inside the app, the browser mock
// elsewhere; api/client.ts) and renders the app.
import { render } from "preact";
import { App } from "./App";
import { getApi } from "./api/client";
import "./style.css";
// The app inside the jukebox look; after style.css so its rules win (see the file).
import "./skin-app.css";

// Taken before anything renders; reported by the --smoke-test (ms since time origin).
const scriptStartMs = performance.now();

const root = document.getElementById("app");
if (root) {
  void getApi().then((api) => render(<App api={api} scriptStartMs={scriptStartMs} />, root));
}
