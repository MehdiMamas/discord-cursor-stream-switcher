# Changelog

## 0.2.0 (2026-10-06)

- Ships as an installer (`cursor-stream-switcher-setup.exe`) instead of a bare `.exe`. It installs to Program Files, adds a Start menu entry and an optional "Start with Windows", and is listed in Settings > Apps.
- Uninstalling removes the virtual display driver and the autostart entry, and offers to delete settings and logs.
- Updates download the signed installer and run it (one admin prompt). It upgrades in place and restarts the app.
- 0.1.x copies can't update to this release by themselves (it has no bare `.exe`); install it once by hand.

## 0.1.1 (2026-10-06)

First published release (the 0.1.0 build failed at signing and was never released).

- Release signing loads the unencrypted key, and the release job checks the signature against the app's built-in key before publishing.

## 0.1.0 (2026-10-06)

First release.

- Streams the monitor under the mouse to a virtual "Stream Cursor" display for Discord screen share.
- One-click install of the signed Virtual Display Driver (25.7.23), resolution picker, uninstall.
- Pause and lock hotkeys, per-monitor "hide from stream", cursor drawing, mouse guard.
- Moves the stream display out from between monitors automatically.
- Signed self-updates from GitHub Releases.
