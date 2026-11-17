// SPDX-License-Identifier: GPL-3.0-or-later
// Jukeoke desktop app entry point. Everything lives in the library (src/lib.rs).
//
// Release builds use the Windows GUI subsystem, so no console window opens when the
// app is started with a double-click from the USB stick.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The dialog plugin is registered here rather than in the library: its message
    // dialogs import TaskDialogIndirect (Common Controls v6), which only executables
    // with our application manifest can load, and the library's unit-test executable
    // has no manifest (see build.rs).
    std::process::exit(jukeoke_lib::run(|builder| builder.plugin(tauri_plugin_dialog::init())));
}
