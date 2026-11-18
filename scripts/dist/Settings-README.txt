Settings
========

Empty on a fresh stick. @NAME@ creates what it needs here, so that nothing is written
to the computer it runs on:

  settings.json               your settings (the key and mix of each song are saved
                              inside the song file itself)
  selfcheck-<date time>.txt   self-check reports: bring these back if something fails
  logs\                       logs, including startup-error.txt if the app could not start
  webview-windows\            the Windows web view's working files
  cache\, tmp\                the Studio's caches and temporary files (Windows only)

Deleting everything in this folder resets the app. Songs, recordings and models are
not affected.
