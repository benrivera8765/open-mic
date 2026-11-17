// SPDX-License-Identifier: GPL-3.0-or-later
// Jukeoke desktop app entry point. Everything lives in the library (src/lib.rs).
//
// Release builds use the Windows GUI subsystem, so no console window opens when the
// app is started with a double-click from the USB stick.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::process::exit(jukeoke_lib::run());
}
