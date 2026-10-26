# Kura

Native Windows Bitwarden-compatible client: GPUI Kit UI on the Bitwarden Rust SDK,
replacing the Electron client (one process, no Chromium/Angular/WASM, vault window
destroyed while sitting in the tray).

## Crates

- `kura-core` - UI-independent vault engine over `bitwarden-pm` (pinned
  `bitwarden/sdk-internal` revision). GPL-3.0.
- `kura-app` - GPUI Kit shell.

## Status

Milestone M1: sign in with the master password and a TOTP or email code, sync,
and a searchable read-only list. The UI speaks Japanese and English.

Details, copy, TOTP display, locking, editing, the tray and the settings screen
are not in yet.

## Build

Everything runs through mise (`mise install`; versions and checksums are pinned in
`mise.lock`).

```powershell
mise run run      # cargo run -p kura-app
mise run check    # cargo check --workspace --all-targets
mise run build    # release build
```
