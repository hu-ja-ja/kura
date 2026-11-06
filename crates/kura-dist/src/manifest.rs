//! The manifest: one signed release, no history.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::Error;

/// The largest manifest accepted from the network. The release notes are the
/// only field that grows, and they stay a few kilobytes.
pub(crate) const MAX_MANIFEST: usize = 256 * 1024;

/// The largest installer accepted. The manifest states the size anyway.
pub(crate) const MAX_ARTIFACT: u64 = 512 * 1024 * 1024;

/// A release version. Only strict `x.y.z` is a release, and the ordering
/// compares each part numerically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let version = Self {
            major: parts.next()?.parse().ok()?,
            minor: parts.next()?.parse().ok()?,
            patch: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| DeError::custom(format!("{text} is not an x.y.z version")))
    }
}

/// The newest release, as published in the manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
    pub signature: String,
}

impl Release {
    pub fn new(version: Version, url: String, size: u64, sha256: String, notes: String) -> Self {
        Self {
            version,
            url,
            size,
            sha256,
            notes,
            signature: String::new(),
        }
    }

    /// Signs the manifest. The field order is part of the format; the notes
    /// come last so they cannot change how the earlier fields read.
    pub fn sign(&mut self, key: &SigningKey) {
        self.signature = BASE64.encode(key.sign(self.payload().as_bytes()).to_bytes());
    }

    pub fn from_json(bytes: &[u8], key: &VerifyingKey) -> Result<Self, Error> {
        let release: Self = serde_json::from_slice(bytes)
            .map_err(|error| Error::Manifest(format!("the manifest is not valid JSON: {error}")))?;
        release.verify(key)?;
        Ok(release)
    }

    pub(crate) fn validate(&self, secure: bool) -> Result<(), Error> {
        if self.size == 0 || self.size > MAX_ARTIFACT {
            return Err(Error::Manifest(format!(
                "the manifest states an impossible size: {}",
                self.size
            )));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Manifest(
                "the manifest's SHA-256 is not 64 hex digits".to_owned(),
            ));
        }
        let url = Url::parse(&self.url)
            .map_err(|error| Error::Manifest(format!("the manifest's URL is invalid: {error}")))?;
        if secure && url.scheme() != "https" {
            return Err(Error::Manifest(format!(
                "the manifest points at a plain-text URL: {url}"
            )));
        }
        Ok(())
    }

    fn payload(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}",
            self.version, self.url, self.size, self.sha256, self.notes
        )
    }

    fn verify(&self, key: &VerifyingKey) -> Result<(), Error> {
        let signature = BASE64
            .decode(self.signature.trim())
            .map_err(|error| Error::Signature(format!("the signature is not base64: {error}")))?;
        let signature: [u8; 64] = signature
            .try_into()
            .map_err(|_| Error::Signature("the signature is not 64 bytes".to_owned()))?;
        key.verify_strict(
            self.payload().as_bytes(),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| Error::Signature("the manifest signature does not match".to_owned()))
    }
}

pub(crate) fn verifying_key(public_key: &str) -> Result<VerifyingKey, Error> {
    let bytes = BASE64
        .decode(public_key.trim())
        .map_err(|error| Error::Configuration(format!("the public key is not base64: {error}")))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Configuration("the public key is not 32 bytes".to_owned()))?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|error| Error::Configuration(format!("the public key is invalid: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_three_numbers() {
        assert_eq!(Version::parse("0.2.0"), Some(Version::new(0, 2, 0)));
        assert!(Version::parse("0.2").is_none());
        assert!(Version::parse("0.2.0.0").is_none());
        assert!(Version::parse("v0.2.0").is_none());
        assert!(Version::new(0, 10, 0) > Version::new(0, 9, 9));
    }

    #[test]
    fn the_manifest_round_trips_through_json() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut release = Release::new(
            Version::new(1, 2, 3),
            "https://example.invalid/Kura-1.2.3-setup.exe".to_owned(),
            42,
            "a".repeat(64),
            "- note".to_owned(),
        );
        release.sign(&key);
        let json = serde_json::to_vec(&release).expect("the manifest serializes");
        assert_eq!(
            Release::from_json(&json, &key.verifying_key()).expect("the manifest verifies"),
            release
        );
    }

    #[test]
    fn one_changed_byte_breaks_the_signature() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut release = Release::new(
            Version::new(1, 2, 3),
            "https://example.invalid/Kura-1.2.3-setup.exe".to_owned(),
            42,
            "a".repeat(64),
            String::new(),
        );
        release.sign(&key);
        release.notes.push('!');
        let json = serde_json::to_vec(&release).expect("the manifest serializes");
        assert!(matches!(
            Release::from_json(&json, &key.verifying_key()),
            Err(Error::Signature(_))
        ));
    }

    #[test]
    fn another_key_does_not_verify() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let mut release = Release::new(
            Version::new(1, 2, 3),
            "https://example.invalid/Kura-1.2.3-setup.exe".to_owned(),
            42,
            "a".repeat(64),
            String::new(),
        );
        release.sign(&key);
        let json = serde_json::to_vec(&release).expect("the manifest serializes");
        assert!(matches!(
            Release::from_json(&json, &other.verifying_key()),
            Err(Error::Signature(_))
        ));
    }
}
