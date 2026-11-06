//! The engine against a loopback feed: the real fetch, verification and
//! download code with only the network handed to the test.

use std::cell::Cell;
use std::io::{self, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::SigningKey;
use kura_dist::{Release, Updater, Version};
use sha2::{Digest as _, Sha256};
use url::Url;

const CURRENT: &str = "0.0.9";
const SEED: [u8; 32] = [42; 32];

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&SEED)
}

fn public_key(key: &SigningKey) -> String {
    BASE64.encode(key.verifying_key().to_bytes())
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

/// A loopback HTTP server that serves `(path, body)` routes.
struct Loopback {
    address: SocketAddr,
    _server: thread::JoinHandle<()>,
}

impl Loopback {
    fn new(routes: impl FnOnce(SocketAddr) -> Vec<(String, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback server");
        let address = listener.local_addr().expect("the loopback address");
        let routes = routes(address);
        let server = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // A client that connects without requesting must not block
                // every later request.
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Ok(path) = request_path(&mut stream) else {
                    continue;
                };
                let route = routes.iter().find(|(route, _)| *route == path);
                let body = route.map_or(&[][..], |(_, body)| body.as_slice());
                let status = if route.is_some() {
                    "200 OK"
                } else {
                    "404 Not Found"
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        Self {
            address,
            _server: server,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    fn updater(&self, key: &SigningKey) -> Updater {
        let feed = Url::parse(&self.url("/manifest.json")).expect("a feed URL");
        Updater::new(feed, &public_key(key)).expect("an updater for the loopback feed")
    }
}

fn request_path(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let request = String::from_utf8_lossy(&request);
    let target = request.split_whitespace().nth(1).unwrap_or("/");
    Ok(target.split('?').next().unwrap_or("/").to_owned())
}

/// Serves a manifest for `artifact` (named after `version`) and returns the
/// updater plus the server keeping it alive.
fn served(version: &str, artifact: &[u8]) -> (Updater, Loopback) {
    let key = signing_key();
    let mut release = Release::new(
        Version::parse(version).expect("a version"),
        String::new(),
        artifact.len() as u64,
        hex(&Sha256::digest(artifact)),
        "- Faster search".to_owned(),
    );
    let server = Loopback::new(|address| {
        release.url = format!("http://{address}/{}", artifact_name(version));
        release.sign(&key);
        vec![
            (
                "/manifest.json".to_owned(),
                serde_json::to_vec(&release).expect("the manifest serializes"),
            ),
            (format!("/{}", artifact_name(version)), artifact.to_vec()),
        ]
    });
    (server.updater(&key), server)
}

fn artifact_name(version: &str) -> String {
    format!("Kura-{version}-setup.exe")
}

#[test]
fn a_signed_release_is_downloaded_and_verified() {
    let artifact = b"the published installer bytes".to_vec();
    let (updater, _server) = served("0.1.0", &artifact);
    let current = Version::parse(CURRENT).expect("the current version");

    let release = updater
        .check(current)
        .expect("the check succeeds")
        .expect("an update is available");
    assert_eq!(release.version, Version::new(0, 1, 0));
    assert_eq!(release.notes, "- Faster search");

    let dir = tempfile::tempdir().expect("a temporary directory");
    let received = Cell::new(0);
    let path = updater
        .download(&release, dir.path(), |downloaded, total| {
            assert_eq!(total, artifact.len() as u64);
            received.set(downloaded);
        })
        .expect("the download verifies");
    assert_eq!(std::fs::read(&path).expect("read the download"), artifact);
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("Kura-0.1.0-setup.exe")
    );
    assert_eq!(received.get(), artifact.len() as u64);
}

#[test]
fn a_release_older_than_the_running_build_is_not_an_update() {
    let artifact = b"an older release".to_vec();
    let (updater, _server) = served(CURRENT, &artifact);

    let newer = Version::new(0, 1, 0);
    assert_eq!(updater.check(newer).expect("the check succeeds"), None);
}

#[test]
fn a_manifest_signed_by_another_key_is_refused() {
    let artifact = b"the published installer bytes".to_vec();
    let (_, server) = served("0.1.0", &artifact);
    let other = SigningKey::from_bytes(&[9; 32]);
    let updater = server.updater(&other);

    assert!(matches!(
        updater.check(Version::parse(CURRENT).expect("the current version")),
        Err(kura_dist::Error::Signature(_))
    ));
}

#[test]
fn a_tampered_artifact_is_discarded() {
    let published = b"the published installer bytes".to_vec();
    // The manifest keeps the hash of the published bytes; the server hands out
    // a file of the same length with one changed byte.
    let mut tampered = published.clone();
    tampered[0] ^= 0xff;
    let key = signing_key();
    let mut release = Release::new(
        Version::new(0, 1, 0),
        String::new(),
        published.len() as u64,
        hex(&Sha256::digest(&published)),
        String::new(),
    );
    let server = Loopback::new(|address| {
        release.url = format!("http://{address}/{}", artifact_name("0.1.0"));
        release.sign(&key);
        vec![
            (
                "/manifest.json".to_owned(),
                serde_json::to_vec(&release).expect("the manifest serializes"),
            ),
            (format!("/{}", artifact_name("0.1.0")), tampered.clone()),
        ]
    });
    let updater = server.updater(&key);
    let release = updater
        .check(Version::new(0, 0, 9))
        .expect("the check succeeds")
        .expect("an update is available");

    let dir = tempfile::tempdir().expect("a temporary directory");
    assert!(matches!(
        updater.download(&release, dir.path(), |_, _| {}),
        Err(kura_dist::Error::Signature(_))
    ));
    assert!(!has_files(dir.path()));
}

#[test]
fn a_truncated_download_is_discarded() {
    let artifact = b"the published installer bytes".to_vec();
    let key = signing_key();
    let mut release = Release::new(
        Version::new(0, 1, 0),
        String::new(),
        artifact.len() as u64,
        hex(&Sha256::digest(&artifact)),
        String::new(),
    );
    let half = artifact[..artifact.len() / 2].to_vec();
    let server = Loopback::new(|address| {
        release.url = format!("http://{address}/{}", artifact_name("0.1.0"));
        release.sign(&key);
        vec![
            (
                "/manifest.json".to_owned(),
                serde_json::to_vec(&release).expect("the manifest serializes"),
            ),
            (format!("/{}", artifact_name("0.1.0")), half.clone()),
        ]
    });
    let updater = server.updater(&key);
    let release = updater
        .check(Version::new(0, 0, 9))
        .expect("the check succeeds")
        .expect("an update is available");

    let dir = tempfile::tempdir().expect("a temporary directory");
    assert!(matches!(
        updater.download(&release, dir.path(), |_, _| {}),
        Err(kura_dist::Error::Download(_))
    ));
    assert!(!has_files(dir.path()));
}

fn has_files(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(true)
}
