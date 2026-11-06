English | [日本語](README.ja.md)

# Kura

[![Latest release](https://img.shields.io/github/v/release/hu-ja-ja/kura?style=flat-square)](https://github.com/hu-ja-ja/kura/releases/latest)
[![License](https://img.shields.io/github/license/hu-ja-ja/kura?style=flat-square)](LICENSE)

Native Windows client for Bitwarden.

## Features

- **Sign-in:** Master password, TOTP and email two-step verification; saved-login unlock.
- **Vault:** Search, folders, favorites, and item details.
- **Actions:** Copy credentials and TOTP codes; open URIs.
- **Desktop:** System tray, auto-lock, autostart, and single-instance support.
- **Security:** Protected saved login, clipboard auto-clear after 30 seconds, and screen capture protection.
- **Updates:** Signed in-app updates.

## Requirements

| Requirement     | Supported                            |
| --------------- | ------------------------------------ |
| Windows         | Windows 10 22H2 or later; Windows 11 |
| Architecture    | x64                                  |
| Display scaling | 100% to 200%                         |
| Input           | Japanese IME                         |

## Download and install

1. Download the latest `Kura-<version>-setup.exe` from [Releases](https://github.com/hu-ja-ja/kura/releases/latest).
2. Run the installer. It installs for the current user and does not require administrator rights.
3. Choose an install folder. The installer appends `Kura`; the optional desktop shortcut is off by default.

The installer is unsigned. Windows SmartScreen may show a warning.

## Updates

Kura checks for updates 10 seconds after launch and every 24 hours. You can also check manually.
Updates are installed when Kura exits.

## Uninstall

1. Uninstall Kura from Windows Settings > Apps > Installed apps, or run `unins000.exe` from the install folder.
2. Choose whether to delete the user data:

   | Location              | Contents                                      |
   | --------------------- | --------------------------------------------- |
   | `%APPDATA%\Kura`      | Settings, saved login, and update preferences |
   | `%LOCALAPPDATA%\Kura` | Logs and staged updates                       |

Choose No to keep the data for a later reinstall.

## Roadmap

- [ ] Add vault items.
- [ ] Edit vault item fields.
- [ ] Delete items and move them to Trash.
- [ ] Biometric unlock bridge.
- [ ] Attachments.
- [ ] Import vault data.
- [ ] Multiple accounts.
- [ ] More language support.

## Support

Report bugs or request features on [GitHub Issues](https://github.com/hu-ja-ja/kura/issues). Logs are stored in `%LOCALAPPDATA%\Kura\logs` and do not contain passwords, tokens, or vault contents.

## Build from source

Install tools and run project tasks with mise. Versions and checksums are pinned in `mise.lock`.

```powershell
mise install
mise run run      # cargo run -p kura-app
mise run check    # cargo check --workspace --all-targets
mise run lint     # cargo fmt --check and clippy
mise run build    # release build
mise run package  # Inno Setup installer
```

`mise run package` needs Inno Setup 7.1.0; `mise run install-inno` installs it silently. It
writes `dist/Kura-<version>-setup.exe` with its SHA-256 file. Run `cargo test` for the
workspace tests, `mise run audit` to audit dependencies, and `mise run licenses` to
regenerate the bundled license list after changing dependencies.

A development build uses the public test key and refuses to install updates.

## Crate layout

| Crate           | Purpose                                                                                 |
| --------------- | --------------------------------------------------------------------------------------- |
| `kura-core`     | UI-independent vault engine over the Bitwarden SDK                                      |
| `kura-app`      | GPUI Kit application                                                                    |
| `kura-platform` | Windows tray, single instance, lock detection, autostart, and screen capture protection |
| `kura-dist`     | Update engine and release tool                                                          |

## License

- Kura: [GPL-3.0-only](LICENSE).
- Font: IBM Plex Sans JP, SIL Open Font License 1.1. See `crates/kura-app/assets/fonts/license.txt`.
- Third-party notices: `THIRD-PARTY-NOTICES.txt`. Dependency licenses are available in Settings.

## Disclaimer

Kura uses the Bitwarden SDK. It is an unofficial client and is not affiliated with Bitwarden.
