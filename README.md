# Kura

Native Windows Bitwarden-compatible client: GPUI Kit UI on the Bitwarden Rust SDK,
replacing the Electron client (one process, no Chromium/Angular/WASM, vault window
destroyed while sitting in the tray).

## Crates

- `kura-core` - UI-independent vault engine over `bitwarden-pm` (pinned
  `bitwarden/sdk-internal` revision). GPL-3.0.
- `kura-app` - GPUI Kit shell.

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
