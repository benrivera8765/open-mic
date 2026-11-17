// SPDX-License-Identifier: GPL-3.0-or-later
// `tauri icon` also writes Android, iOS and Windows Store images. Jukeoke ships only
// for Windows and macOS (WORK_ORDER section 12), so keep just what tauri.conf.json
// lists (plus icon.png for the Linux-style window icon) and delete the rest.
import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const iconsDir = fileURLToPath(new URL("../../src-tauri/icons/", import.meta.url));
const keep = new Set(["source", "32x32.png", "64x64.png", "128x128.png", "128x128@2x.png", "icon.icns", "icon.ico", "icon.png"]);
for (const name of readdirSync(iconsDir)) {
  if (!keep.has(name)) rmSync(join(iconsDir, name), { recursive: true, force: true });
}
console.log(`icons kept: ${[...readdirSync(iconsDir)].join(", ")}`);
