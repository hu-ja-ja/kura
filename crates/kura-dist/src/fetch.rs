//! HTTP: one client, bounded reads, and plain HTTP only where the feed is.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::manifest::MAX_MANIFEST;
use crate::{Error, Release, hex};

const FEED_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIRECTS: usize = 5;
const CHUNK: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct Http {
    client: Client,
    secure: bool,
}

impl Http {
    pub(crate) fn new(secure: bool) -> Result<Self, Error> {
        // reqwest is built with `rustls-no-provider`, so the provider has to be
        // installed before the first client is built. A process that already
        // installed one keeps it.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= REDIRECTS
                    || (secure && attempt.url().scheme() != "https")
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .user_agent(concat!("Kura/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                Error::Configuration(format!("the HTTP client could not be built: {error}"))
            })?;
        Ok(Self { client, secure })
    }

    pub(crate) fn is_secure(&self) -> bool {
        self.secure
    }

    pub(crate) fn fetch(&self, url: &Url) -> Result<Vec<u8>, Error> {
        let mut response = self.open(url, FEED_TIMEOUT, Error::Manifest)?;
        let mut body = Vec::new();
        response
            .by_ref()
            .take(MAX_MANIFEST as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|error| Error::Manifest(format!("{url} could not be read: {error}")))?;
        if body.len() > MAX_MANIFEST {
            return Err(Error::Manifest(format!(
                "{url} returned a feed that is too large"
            )));
        }
        Ok(body)
    }

    pub(crate) fn download(
        &self,
        release: &Release,
        staging: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, Error> {
        let url = Url::parse(&release.url)
            .map_err(|error| Error::Download(format!("the download URL is invalid: {error}")))?;
        let mut response = self.open(&url, TRANSFER_TIMEOUT, Error::Download)?;
        std::fs::create_dir_all(staging).map_err(|error| {
            Error::Download(format!(
                "{} could not be created: {error}",
                staging.display()
            ))
        })?;

        let artifact = staging.join(format!("Kura-{}-setup.exe", release.version));
        let partial = artifact.with_extension("exe.part");
        // Leftovers from earlier versions would otherwise pile up in the
        // update folder.
        if let Ok(entries) = std::fs::read_dir(staging) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path != artifact && path != partial {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        let mut file = std::fs::File::create(&partial).map_err(|error| {
            Error::Download(format!(
                "{} could not be written: {error}",
                partial.display()
            ))
        })?;

        let mut hasher = Sha256::new();
        let mut received = 0u64;
        let mut chunk = [0u8; CHUNK];
        let outcome: Result<(), Error> = (|| {
            loop {
                let read = response.read(&mut chunk).map_err(|error| {
                    Error::Download(format!("{url} failed mid-download: {error}"))
                })?;
                if read == 0 {
                    break;
                }
                received += read as u64;
                if received > release.size {
                    return Err(Error::Download(
                        "the download is larger than the manifest says".to_owned(),
                    ));
                }
                hasher.update(&chunk[..read]);
                file.write_all(&chunk[..read]).map_err(|error| {
                    Error::Download(format!(
                        "{} could not be written: {error}",
                        partial.display()
                    ))
                })?;
                progress(received, release.size);
            }
            Ok(())
        })();

        drop(file);
        outcome.map_err(|error| discard(&partial, error))?;
        if received != release.size {
            return Err(discard(
                &partial,
                Error::Download(format!(
                    "the download holds {received} of {} bytes",
                    release.size
                )),
            ));
        }
        if !release
            .sha256
            .eq_ignore_ascii_case(&hex(&hasher.finalize()))
        {
            return Err(discard(
                &partial,
                Error::Signature("the downloaded file does not match the manifest".to_owned()),
            ));
        }

        // A leftover file from the same version would make the rename fail on
        // Windows; it is about to be replaced anyway.
        let _ = std::fs::remove_file(&artifact);
        std::fs::rename(&partial, &artifact).map_err(|error| {
            discard(
                &partial,
                Error::Download(format!("{} could not be kept: {error}", artifact.display())),
            )
        })?;
        Ok(artifact)
    }

    fn open(
        &self,
        url: &Url,
        timeout: Duration,
        error: fn(String) -> Error,
    ) -> Result<Response, Error> {
        if self.secure && url.scheme() != "https" {
            return Err(error(format!("refusing the plain-text URL {url}")));
        }
        let response = self
            .client
            .get(url.clone())
            .timeout(timeout)
            .send()
            .map_err(|detail| error(format!("{url} could not be fetched: {detail}")))?;
        if !response.status().is_success() {
            return Err(error(format!(
                "{url} answered with HTTP {}",
                response.status()
            )));
        }
        Ok(response)
    }
}

fn discard(path: &Path, error: Error) -> Error {
    let _ = std::fs::remove_file(path);
    error
}
