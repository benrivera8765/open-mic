The Mac app goes in this folder
===============================

This folder is empty because the Mac app cannot be built on the Windows PC: Apple's
compiler and signing tools only run on macOS. When you have it, this folder holds:

    Mac/@NAME@.app          the app
    Mac/Tools/sck_probe     a small recorder test tool (optional)

There are two ways to get it.


A. Build it on the MacBook (no GitHub needed)
---------------------------------------------

  1. Copy the project's source folder (the "@REPO_DIR@" folder with the code, not this
     stick folder) to the Mac.
  2. Open Terminal, go into that folder and run:

         bash scripts/build-mac.sh

     The first time it takes 5 to 20 minutes and needs about 3 GB of free space and
     an internet connection. If Apple's Command Line Tools are missing, it opens
     Apple's installer and stops: run the command again when the installer is done.
     It installs Rust and Node.js for your user account only if they are missing,
     never asks for your password (no sudo) and changes no system settings.
  3. The finished app is in the project folder at

         dist/Mac/@NAME@.app       (and dist/Mac/Tools/sck_probe)

  4. In Finder, copy @NAME@.app (and the Tools folder) into this Mac folder on the
     stick.

  An app built on your own Mac is not quarantined, so it opens with a plain
  double-click, also from the stick.


B. Download it from GitHub (once the project is on GitHub)
-----------------------------------------------------------

  The project's GitHub workflow (.github/workflows/ci.yml) builds the Mac app on every
  push, on GitHub's macOS machines. GitHub is not set up yet; once it is:

  1. On the repository page open Actions, the latest "CI" run, and download the
     artifact "macos-results".
  2. Unzip it on the Mac (double-click in Finder), then double-click
     "@NAME@-mac-arm64.zip" inside it. Unzip on the Mac, not on Windows: unzipping on
     Windows can break the app's signature.
  3. In Finder, copy @NAME@.app into this Mac folder on the stick.

  A downloaded app is quarantined by macOS. The first time, in Finder, Control-click
  the app, choose Open, then click Open again. Or, in Terminal, once:

      xattr -dr com.apple.quarantine "/Volumes/<name of the stick>/Mac/@NAME@.app"


Then
----

Open Mac/@NAME@.app from the stick. The app finds Library, Recordings and Settings
next to the Mac folder by itself. Run the self-check (button at the top right) the
first time: it writes Settings/selfcheck-<date and time>.txt on the stick. Bring that
report back to the Windows PC.

The Mac app needs macOS 13 Ventura or later on an Apple Silicon Mac (M1 or newer). It
plays songs, changes the key, edits lyric timing and records; it does not make songs.
It has not been run on a real Mac yet.
