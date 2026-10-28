# Cursor Stream Switcher

A Windows tray app (Rust) that adds a virtual monitor called "Stream Cursor" and mirrors whichever real monitor the mouse is on, so one Discord screen share follows the cursor.

## Setup
Windows 10/11, stable Rust (MSVC toolchain), Visual Studio Build Tools. The installer needs Inno Setup 6.

## Commands
- `cargo test`
- `cargo build --release` - produces `target\release\cursor-stream-switcher.exe`.
- `iscc installer\cursor-stream-switcher.iss` - builds `dist\cursor-stream-switcher-setup.exe`.
- `cargo run --release --example probe_fps -- \\.\DISPLAY2 10` - counts real frames a display produces.

## Where things live
- `src/` - one module per concern (`capture`, `mirror`, `monitors`, `driver`, `tray`, `hotkey`, `update`, `config`, ...); entry is `src/main.rs`.
- `examples/` - `probe_fps.rs`, `sign.rs`. `installer/` - Inno Setup script. `scripts/` - icon generation.
- `vendor/vdd/` - bundled Virtual Display Driver files with their license.
- `release-key.pub` - public key the app uses to verify signed updates.

## Conventions
- CI (`.github/workflows/ci.yml`, Windows): `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. Run these before a PR.
- `rustfmt.toml`: max width 110. Rust edition 2024, MSRV 1.85. `.cargo/config.toml` links the C runtime statically.
- Release: bump `version` in `Cargo.toml`, add a line to `CHANGELOG.md`, commit, then tag `vX.Y.Z` and push the tag; the Release workflow tests, builds, signs (secret `MINISIGN_SECRET_KEY`) and publishes.

## Gotchas
- Windows-only; builds elsewhere are not expected to work.
- Updates are signed per version; do not change the signing or verification flow casually.
