//! The release tool as a process: argument parsing, the key environment
//! variables, and the manifest it writes.

use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use kura_dist::{Release, Version};
use sha2::{Digest as _, Sha256};

const ARTIFACT: &[u8] = b"the installer bytes";

fn tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kura-dist"))
}

fn artifact(dir: &Path) -> std::path::PathBuf {
    let path = dir.join(format!("Kura-{}-setup.exe", env!("CARGO_PKG_VERSION")));
    std::fs::write(&path, ARTIFACT).expect("write the artifact");
    path
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    text
}

#[test]
fn sign_writes_a_manifest_that_verifies() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let artifact = artifact(dir.path());
    let key = SigningKey::from_bytes(&[3; 32]);
    let output = dir.path().join("site").join("manifest.json");

    let result = tool()
        .arg("sign")
        .arg("--artifact")
        .arg(&artifact)
        .arg("--url-prefix")
        .arg("https://example.invalid/releases/")
        .arg("--output")
        .arg(&output)
        .env("ED25519_PRIVATE_KEY", BASE64.encode([3u8; 32]))
        .env(
            "ED25519_PUBLIC_KEY",
            BASE64.encode(key.verifying_key().to_bytes()),
        )
        .output()
        .expect("run kura-dist");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let release = Release::from_json(
        &std::fs::read(&output).expect("read the manifest"),
        &key.verifying_key(),
    )
    .expect("the signed manifest verifies");
    assert_eq!(
        release.version,
        Version::parse(env!("CARGO_PKG_VERSION")).expect("a version")
    );
    assert_eq!(
        release.url,
        format!(
            "https://example.invalid/releases/Kura-{}-setup.exe",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(release.size, ARTIFACT.len() as u64);
    assert_eq!(release.sha256, hex(&Sha256::digest(ARTIFACT)));
}

#[test]
fn package_rejects_an_unknown_argument() {
    let result = tool()
        .arg("package")
        .arg("--nope")
        .output()
        .expect("run kura-dist");
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("unexpected argument --nope"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn sign_refuses_a_public_key_that_does_not_belong() {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let artifact = artifact(dir.path());
    let other = SigningKey::from_bytes(&[4; 32]);
    let output = dir.path().join("manifest.json");

    let result = tool()
        .arg("sign")
        .arg("--artifact")
        .arg(&artifact)
        .arg("--url-prefix")
        .arg("https://example.invalid/releases/")
        .arg("--output")
        .arg(&output)
        .env("ED25519_PRIVATE_KEY", BASE64.encode([3u8; 32]))
        .env(
            "ED25519_PUBLIC_KEY",
            BASE64.encode(other.verifying_key().to_bytes()),
        )
        .output()
        .expect("run kura-dist");
    assert!(!result.status.success());
    assert!(!output.exists());
}
