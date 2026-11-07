# Security Policy

Kura holds vault credentials on Windows, so security reports are welcome.

## Supported versions

Only the latest release is supported. Kura is pre-1.0 and has no maintenance branches.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:

<https://github.com/hu-ja-ja/kura/security/advisories/new>

Without a GitHub account, email <hujaja.jp@gmail.com> instead; replies there may be slower.

Do not open a public issue. Do not paste real vault data, tokens, or your master password; a description is enough and details can follow in the private thread.

Include the Kura version, the Windows build, and the steps that reproduce the issue, and say whether the latest release still shows it.

## Safe harbour

Testing your own Kura installation with your own account is authorised, and good-faith research on that basis will not be met with legal action. Do not test against other people's vaults or servers, and do not run denial-of-service or spam tests.

## What to expect

Kura is an unpaid, single-maintainer project, so there is no SLA. The aim is to acknowledge a report within 7 days.

Fixes are generally shipped in the next available release, depending on severity and complexity. Once a fix is available, a GitHub Security Advisory is published with credit unless you ask to stay anonymous. The reporter and maintainer will coordinate a disclosure date where practical. You are free to publish your own write-up once the fix is released, or after that date if the fix takes longer. A CVE may be requested through the same advisory when appropriate, depending on the nature and impact of the vulnerability.

## In scope

Vulnerabilities that can expose credentials or compromise Kura's security guarantees without requiring arbitrary code execution in the user's session:

- Saved-login protection (DPAPI) and its temporary files
- Update manifest signature verification (Ed25519)
- Credential leakage through logs, crash dumps, temporary files, or the clipboard
- Screen-capture protection, including bypasses of Windows capture-exclusion mechanisms against unprivileged applications
- Installer behaviour
- Access from another non-admin Windows user, a stolen disk, or a stolen backup

The saved login is sealed to the Windows account, not to the Kura process: another process running as the same user can open it, as with Bitwarden Desktop.

## Out of scope

- Attacks requiring physical access to an unlocked device, administrator rights, or code execution in the user's session. This also includes privileged processes reading screen or memory. No password manager can fully protect against an attacker who already runs code as you.
- SmartScreen warnings from the unsigned installer, which the README documents
- Bugs in the Bitwarden server, Vaultwarden, or another self-hosted server; report those upstream
- Vulnerabilities in upstream dependencies without a Kura-specific security impact. Reports that Kura uses or pins a vulnerable dependency are still welcome
