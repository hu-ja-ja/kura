# Kura

Native Windows Bitwarden-compatible client: GPUI Kit UI on the Bitwarden Rust SDK,
replacing the Electron client (one process, no Chromium/Angular/WASM, vault window
destroyed while sitting in the tray).

## Status

M0 to M3 are implemented: sign-in with TOTP and email two-factor, sync, list,
search, detail, copy, TOTP, lock, tray, settings, auto-lock, autostart and the
single-instance guard. While the window sits in the tray it is destroyed, the
DirectX devices are released and Windows is asked to trim the working set; the
app stays one process. Sign-in still needs a real account check.

## Crates

- `kura-core` - UI-independent vault engine over `bitwarden-pm` (pinned
  `bitwarden/sdk-internal` revision). GPL-3.0.
- `kura-app` - GPUI Kit shell.
- `kura-platform` - Win32 tray, single instance, idle and session lock detection,
  autostart and screen capture protection.

## Fonts

The UI uses IBM Plex Sans JP (Regular and SemiBold), bundled under the SIL Open
Font License 1.1. The license text is in `crates/kura-app/assets/fonts/license.txt`.

## Build

Everything runs through mise (`mise install`; versions and checksums are pinned in
`mise.lock`).

```powershell
mise run run      # cargo run -p kura-app
mise run check    # cargo check --workspace --all-targets
mise run build    # release build
```
