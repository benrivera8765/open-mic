// SPDX-License-Identifier: GPL-3.0-or-later
// Entry point of the web UI.
import { render } from "preact";
import { App } from "./App";
import "./style.css";

// Taken before anything renders; reported by the smoke test (ms since time origin).
const scriptStartMs = performance.now();

const root = document.getElementById("app");
if (root) {
  render(<App scriptStartMs={scriptStartMs} />, root);
}
