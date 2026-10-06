# Cursor Stream Switcher

Share **one screen** in Discord, and it always shows **the monitor your mouse is on**.

Discord can only stream one fixed screen. This app adds a virtual monitor called
**"Stream Cursor"** and keeps drawing whichever real monitor your cursor is on onto it.
You share "Stream Cursor" in Discord once. As you move between monitors, the stream follows.
Because you share a *screen* (not a window), Discord also streams your system audio.

```
 Monitor 1        Monitor 2        Monitor 3            "Stream Cursor" (virtual)
┌─────────┐      ┌─────────┐      ┌─────────┐          ┌──────────────────────┐
│         │      │   🖱️    │      │         │  ──────► │  copy of Monitor 2   │ ──► Discord
└─────────┘      └─────────┘      └─────────┘          └──────────────────────┘
```

- **Light:** one 1.3 MB `.exe`, no runtime to install. About 0.3% GPU and almost no CPU in
  normal use. See [Performance](#performance).
- **Private:** no accounts, no telemetry, no analytics. The only network request is an
  optional daily update check to this repo's GitHub Releases. See [Privacy and security](#privacy-and-security).
- **Works with any layout:** 2, 3 or more monitors, different resolutions and scaling,
  rotated screens, HDR screens, and monitors on different GPUs.

## Install

1. Download `cursor-stream-switcher.exe` from the [latest release](https://github.com/MehdiMamas/discord-cursor-stream-switcher/releases/latest).
2. Put it in a folder you own, e.g. `%LOCALAPPDATA%\Programs\CursorStreamSwitcher\`, so it
   can update itself. Avoid `Program Files`.
3. Run it. A tray icon appears (a little blue screen with a cursor).
4. Click the **"One-time setup"** notification, or open the tray menu and choose
   **Virtual display → Install driver...**.
   - Approve the Windows admin prompt.
   - Windows then asks whether to install display software from **SignPath Foundation**.
     That is the signed [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver)
     (MIT). Click **Install**.
5. Optional: tray menu → **Start with Windows**.

Windows SmartScreen may warn on first run because the app isn't code-signed yet
(**More info → Run anyway**). Release files are signed for the updater, see below.

## Use it in Discord

1. Join a voice channel → **Share Your Screen** → **Screens**.
2. Pick the screen that shows a copy of your current monitor (the 1920×1080 one). Windows
   calls it **Stream Cursor**.
3. Move the mouse between monitors. After 150 ms on a new monitor, the stream switches to it.

| Action | How |
| --- | --- |
| Pause the stream (shows a "Stream paused" card) | `Ctrl+Alt+P` or tray menu |
| Lock the stream to the current monitor | `Ctrl+Alt+L` or tray menu |
| Never show a monitor (shows a "This screen is private" card) | tray menu → **Hide from stream** |
| Change the stream resolution (720p / 1080p / 1440p / 4K) | tray menu → **Virtual display** |
| Hide or show the mouse cursor in the stream | tray menu → **Show mouse cursor** |

The mouse can't wander onto the "Stream Cursor" screen while the app runs, because there's
no real screen there to see it on. If Windows ever puts the stream screen *between* two
monitors, the app moves it to the right-hand edge so it doesn't block the mouse.

## Settings

Tray menu → **Edit settings file** opens `%APPDATA%\CursorStreamSwitcher\config.toml`.
After saving, use **Reload settings**.

| Key | Default | Meaning |
| --- | --- | --- |
| `switch_delay_ms` | `150` | How long the cursor must stay on a monitor before the stream switches. Stops flicker when you brush past an edge. |
| `show_cursor` | `true` | Draw the mouse cursor into the stream. |
| `max_fps` | `60` | Frame cap for the stream screen (10–240). Discord streams at most 60. |
| `keep_mouse_off_stream_display` | `true` | Stop the mouse from entering the virtual screen. |
| `hidden_monitors` | `[]` | Monitor IDs that are never shown. Easier from the tray menu. |
| `hotkey_pause`, `hotkey_lock` | `Ctrl+Alt+P`, `Ctrl+Alt+L` | Any `Ctrl`/`Alt`/`Shift`/`Win` + key, `F1`–`F24`, etc. `""` turns the hotkey off. |
| `check_for_updates` | `true` | Daily check of this repo's releases. |
| `stream_display` | `"auto"` | `auto` finds the virtual display. Or a display name like `'\\.\DISPLAY3'`. |

## Performance

The app only works when something changes:

- It captures **only the monitor you're on**, with Windows Desktop Duplication, on the GPU
  that drives that monitor. The other monitors cost nothing.
- It copies **only the regions that changed** and draws one scaled quad. A still screen means
  no GPU work at all.
- Frames are capped at `max_fps` (60). Higher timer precision is only requested while frames
  are flowing.

Measured on Windows 11 with an RTX 5070, 2560×1440 monitor, 1080p stream:

| Situation | GPU (app) | CPU (app) | Frames on the stream screen |
| --- | --- | --- | --- |
| Normal desktop | ~0.3% | ~1% of one core | only when something changes |
| Mouse moving nonstop | 0.2–0.5% | ~2% of one core | up to ~59 fps |
| Nothing changing | ~0% | ~0% | 0 |

Memory is about 55 MB, mostly the graphics driver. Switching to another monitor takes the
150 ms delay plus 5–50 ms.

## Privacy and security

- **Nothing leaves your PC** except the update check: one HTTPS request a day to
  `github.com/MehdiMamas/discord-cursor-stream-switcher/releases`. It sends no data about
  you beyond what any web request carries (your IP address and the app version in the
  User-Agent). Turn it off with tray menu → **Check for updates daily**.
- **Updates are signed.** Each release `.exe` carries a [minisign](https://jedisct1.github.io/minisign/)
  signature. The app has the public key built in (`release-key.pub`) and rejects any file
  that isn't signed by the release key *for exactly that version*, so a tampered or old file
  can't be slipped in. Download URLs are fixed to this repository.
- **The driver is the official signed build.** The Virtual Display Driver files (release
  25.7.23) are embedded in the `.exe` and pinned by SHA-256 in the tests. Windows checks the
  driver signature itself and asks you before installing it. The app never adds certificates
  to your trusted stores.
- **Its settings can't be changed by other programs.** `C:\VirtualDisplayDriver` (where the
  driver reads its settings) is restricted so only admins can change it.
- **Hidden monitors are never captured**, not just covered up. A paused stream doesn't
  capture anything either.
- Logs stay local: `%LOCALAPPDATA%\CursorStreamSwitcher\app.log`, capped at 1 MB.
- The whole app is under 5,000 lines of Rust in `src/`, tests included, so it can be audited.

## Troubleshooting

- **Discord shows a black screen.** Make sure you picked the right screen. If it's still
  black, toggle Discord's screen-capture option in **Settings → Voice & Video → Screen Share**.
  Video from streaming sites that use DRM always shows black in any screen capture.
- **No "Stream Cursor" screen in Windows.** Tray menu → **Virtual display → Open Display
  settings**: the virtual display must be set to **Extend desktop to this display**.
- **A window disappeared.** It may have opened on the invisible stream screen. Select it in the
  taskbar and press `Win+Shift+←` to bring it back.
- **The cursor gets lost while the app is closed.** The guard only works while the app runs.
  Turn on **Start with Windows**, or remove the driver when you don't need it.
- **Details:** tray menu → **Open log folder** (`app.log`, and `driver.log` for driver setup).

## Uninstall

1. Tray menu → **Virtual display → Remove driver...**
2. Turn off **Start with Windows**, then **Quit**.
3. Delete the `.exe`, `%APPDATA%\CursorStreamSwitcher` and `%LOCALAPPDATA%\CursorStreamSwitcher`.

## Building from source

Requires Windows 10/11, Rust (stable, MSVC) and the Visual Studio Build Tools.

```powershell
cargo test
cargo build --release   # target\release\cursor-stream-switcher.exe
```

`cargo run --release --example probe_fps -- \\.\DISPLAY2 10` counts how many frames a display
really produces, as a screen recorder (or Discord) sees them.

### Releasing

1. Bump `version` in `Cargo.toml`, add a line to `CHANGELOG.md`, commit and push.
2. `git tag v0.2.0 && git push origin v0.2.0`

The **Release** workflow tests, builds, signs the `.exe` with the `MINISIGN_SECRET_KEY` repository
secret, and publishes it with `latest.toml` and `SHA256SUMS.txt`. Running copies pick it up
within a day, or right away from **Check for updates now**.

## Credits

- [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) by
  VirtualDrivers / MikeTheTech (MIT, see `vendor/vdd/LICENSE`).
- Built with [`windows-rs`](https://github.com/microsoft/windows-rs).

MIT License, see [LICENSE](LICENSE).
