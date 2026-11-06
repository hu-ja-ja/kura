//! Release tooling: signs the update manifest, verifies a published feed, and
//! builds the Inno Setup installer. Only maintainers and CI run this; it never
//! ships with the app.

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use kura_dist::{Release, Updater, Version};
use sha2::{Digest as _, Sha256};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const USAGE: &str = "\
usage:
  kura-dist sign --artifact <installer> --url-prefix <url/> [--notes-file <file>] --output <manifest>
  kura-dist verify --feed <url> --public-key <base64> --expect-version <x.y.z>
  kura-dist package [--skip-build]

sign reads ED25519_PRIVATE_KEY and ED25519_PUBLIC_KEY and refuses to sign when
they do not belong together. The version comes from the package version.";

fn main() {
    if let Err(error) = run() {
        eprintln!("kura-dist: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("sign") => sign(args),
        Some("verify") => verify(args),
        Some("package") => package(args),
        _ => Err(USAGE.into()),
    }
}

fn sign(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut artifact = None;
    let mut url_prefix = None;
    let mut notes_file = None;
    let mut output = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--artifact" => artifact = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--url-prefix" => url_prefix = Some(value(&mut args, &arg)?),
            "--notes-file" => notes_file = Some(PathBuf::from(value(&mut args, &arg)?)),
            "--output" => output = Some(PathBuf::from(value(&mut args, &arg)?)),
            other => return Err(format!("unexpected argument {other}\n{USAGE}").into()),
        }
    }
    let artifact = artifact.ok_or("--artifact is required")?;
    let url_prefix = url_prefix.ok_or("--url-prefix is required")?;
    let output = output.ok_or("--output is required")?;
    if !url_prefix.ends_with('/') {
        return Err("--url-prefix must end with a slash".into());
    }
    let version =
        Version::parse(env!("CARGO_PKG_VERSION")).ok_or("the package version is not x.y.z")?;

    let file_name = artifact
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("--artifact needs a file name")?;
    let url = format!("{url_prefix}{file_name}");
    let bytes = std::fs::read(&artifact)
        .map_err(|error| format!("{} could not be read: {error}", artifact.display()))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let notes = match notes_file {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|error| format!("{} could not be read: {error}", path.display()))?,
        None => String::new(),
    };

    let key = signing_key()?;
    let public_key =
        std::env::var("ED25519_PUBLIC_KEY").map_err(|_| "ED25519_PUBLIC_KEY is not set")?;
    let derived = BASE64.encode(key.verifying_key().to_bytes());
    if derived != public_key.trim() {
        return Err("ED25519_PRIVATE_KEY does not belong to ED25519_PUBLIC_KEY".into());
    }

    let mut release = Release::new(version, url, bytes.len() as u64, sha256, notes);
    release.sign(&key);
    let json = serde_json::to_vec_pretty(&release)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, &json)
        .map_err(|error| format!("{} could not be written: {error}", output.display()))?;
    // The published bytes must verify before anything is uploaded.
    Release::from_json(&json, &key.verifying_key())?;

    println!("Manifest: {}", output.display());
    println!("SHA-256:  {}", release.sha256);
    Ok(())
}

fn verify(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut feed = None;
    let mut public_key = None;
    let mut expect = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--feed" => feed = Some(value(&mut args, &arg)?),
            "--public-key" => public_key = Some(value(&mut args, &arg)?),
            "--expect-version" => expect = Some(value(&mut args, &arg)?),
            other => return Err(format!("unexpected argument {other}\n{USAGE}").into()),
        }
    }
    let feed = Url::parse(&feed.ok_or("--feed is required")?)?;
    let public_key = public_key.ok_or("--public-key is required")?;
    let expected = Version::parse(&expect.ok_or("--expect-version is required")?)
        .ok_or("--expect-version is not x.y.z")?;

    let release = Updater::new(feed, &public_key)?.latest()?;
    if release.version != expected {
        return Err(format!(
            "the feed holds {} but {expected} was expected",
            release.version
        )
        .into());
    }
    println!("Verified Kura {} ({} bytes)", release.version, release.size);
    Ok(())
}

fn package(mut args: impl Iterator<Item = String>) -> Result<()> {
    let skip_build = match args.next().as_deref() {
        None => false,
        Some("--skip-build") => true,
        Some(other) => return Err(format!("unexpected argument {other}\n{USAGE}").into()),
    };
    if std::env::var_os("ED25519_PUBLIC_KEY").is_none_or(|value| value.is_empty()) {
        return Err(
            "ED25519_PUBLIC_KEY is missing; the build would not be able to update itself".into(),
        );
    }
    let version =
        Version::parse(env!("CARGO_PKG_VERSION")).ok_or("the package version is not x.y.z")?;
    let root = repository_root()?;
    if !skip_build {
        run_command(
            "cargo",
            &["build", "--release", "--locked", "-p", "kura-app"],
            &root,
        )?;
    }

    let dist = root.join("dist");
    let stage = dist.join("stage");
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;
    for name in [
        "kura.exe",
        "LICENSE",
        "README.md",
        "THIRD-PARTY-NOTICES.txt",
    ] {
        let from = if name == "kura.exe" {
            root.join("target").join("release").join(name)
        } else {
            root.join(name)
        };
        std::fs::copy(&from, stage.join(name))
            .map_err(|error| format!("{} could not be copied: {error}", from.display()))?;
    }

    let iscc = find_iscc().ok_or("ISCC.exe was not found; install Inno Setup 7 or set ISCC")?;
    let defines = [
        format!("/DAppVersion={version}"),
        format!("/DStageDir={}", stage.display()),
        format!("/DOutputDir={}", dist.display()),
        format!(
            "/DIconFile={}",
            root.join("crates")
                .join("kura-app")
                .join("assets")
                .join("kura.ico")
                .display()
        ),
    ];
    let script = root
        .join("crates")
        .join("kura-dist")
        .join("inno")
        .join("kura.iss");
    let status = Command::new(&iscc)
        .args(&defines)
        .arg(&script)
        .current_dir(&root)
        .status()?;
    if !status.success() {
        return Err(format!("ISCC failed with {status}").into());
    }

    let setup = dist.join(format!("Kura-{version}-setup.exe"));
    if !setup.is_file() {
        return Err(format!("the installer was not produced at {}", setup.display()).into());
    }
    let sha256 = hex(&Sha256::digest(std::fs::read(&setup)?));
    std::fs::write(
        dist.join(format!("Kura-{version}-setup.exe.sha256")),
        format!("{sha256}  Kura-{version}-setup.exe\n"),
    )?;
    println!("Installer: {}", setup.display());
    println!("SHA-256:   {sha256}");
    Ok(())
}

/// The signing key is a base64 32-byte seed. It is never taken on the command
/// line, so it cannot end up in a process listing.
fn signing_key() -> Result<SigningKey> {
    let secret =
        std::env::var("ED25519_PRIVATE_KEY").map_err(|_| "ED25519_PRIVATE_KEY is not set")?;
    let bytes = BASE64
        .decode(secret.trim())
        .map_err(|error| format!("ED25519_PRIVATE_KEY is not base64: {error}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "ED25519_PRIVATE_KEY is not a 32-byte seed")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| format!("{name} needs a value").into())
}

fn repository_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "the kura-dist crate is not inside the repository".into())
}

fn run_command(program: &str, args: &[&str], root: &Path) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()?;
    if !status.success() {
        return Err(format!("{program} failed with {status}").into());
    }
    Ok(())
}

fn find_iscc() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ISCC") {
        return Some(PathBuf::from(path));
    }
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        for version in ["7", "6"] {
            candidates.push(
                PathBuf::from(&local)
                    .join("Programs")
                    .join(format!("Inno Setup {version}"))
                    .join("ISCC.exe"),
            );
        }
    }
    for root in ["C:\\Program Files", "C:\\Program Files (x86)"] {
        for version in ["7", "6"] {
            candidates.push(
                Path::new(root)
                    .join(format!("Inno Setup {version}"))
                    .join("ISCC.exe"),
            );
        }
    }
    candidates.into_iter().find(|path| path.is_file())
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
