//! Server URL handling.
//!
//! The SDK wants the identity and the API endpoint separately, so a base URL
//! has to be expanded. Plain HTTP is refused: the SDK only enforces HTTPS in
//! release builds, and the requirement is to refuse it everywhere.

use bitwarden_core::{ClientSettings, DeviceType};
use url::Url;

use crate::error::CoreError;

const USER_AGENT: &str = concat!("Kura/", env!("CARGO_PKG_VERSION"));

/// Build the SDK settings for `server_url`. An empty URL means bitwarden.com.
pub fn client_settings(server_url: Option<&str>) -> Result<ClientSettings, CoreError> {
    let mut settings = ClientSettings {
        user_agent: USER_AGENT.to_string(),
        // The server rejects requests without this header since 2026.
        bitwarden_client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        device_type: DeviceType::WindowsDesktop,
        ..Default::default()
    };

    let raw = server_url.unwrap_or_default().trim();
    if raw.is_empty() {
        return Ok(settings);
    }

    let raw = strip_endpoint_suffix(raw);
    let raw = if raw.contains("://") {
        raw
    } else {
        format!("https://{raw}")
    };

    let url = Url::parse(&raw).map_err(|_| CoreError::InvalidUrl)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CoreError::InvalidUrl);
    }
    match url.scheme() {
        "https" => {}
        "http" => return Err(CoreError::InsecureUrl),
        _ => return Err(CoreError::InvalidUrl),
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(CoreError::InvalidUrl)?
        .to_lowercase();
    match host.as_str() {
        "bitwarden.com" | "www.bitwarden.com" | "vault.bitwarden.com" => return Ok(settings),
        "bitwarden.eu" | "www.bitwarden.eu" | "vault.bitwarden.eu" => {
            settings.identity_url = "https://identity.bitwarden.eu".to_string();
            settings.api_url = "https://api.bitwarden.eu".to_string();
            return Ok(settings);
        }
        _ => {}
    }

    let mut base = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        base.push_str(&format!(":{port}"));
    }
    let path = url.path().trim_end_matches('/');
    if !path.is_empty() {
        base.push_str(path);
    }

    settings.identity_url = format!("{base}/identity");
    settings.api_url = format!("{base}/api");
    Ok(settings)
}

/// Accept a URL that already points at one of the endpoints.
fn strip_endpoint_suffix(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    for suffix in ["/identity", "/api"] {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_url_uses_bitwarden_com() {
        let settings = client_settings(None).unwrap();
        assert_eq!(settings.identity_url, "https://identity.bitwarden.com");
        assert_eq!(settings.api_url, "https://api.bitwarden.com");
    }

    #[test]
    fn the_client_version_is_always_sent() {
        let settings = client_settings(None).unwrap();
        assert_eq!(
            settings.bitwarden_client_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn blank_url_uses_bitwarden_com() {
        let settings = client_settings(Some("   ")).unwrap();
        assert_eq!(settings.api_url, "https://api.bitwarden.com");
    }

    #[test]
    fn bitwarden_com_uses_the_official_endpoints() {
        let settings = client_settings(Some("https://vault.bitwarden.com")).unwrap();
        assert_eq!(settings.identity_url, "https://identity.bitwarden.com");
        assert_eq!(settings.api_url, "https://api.bitwarden.com");
    }

    #[test]
    fn bitwarden_eu_uses_the_eu_endpoints() {
        let settings = client_settings(Some("https://vault.bitwarden.eu")).unwrap();
        assert_eq!(settings.identity_url, "https://identity.bitwarden.eu");
        assert_eq!(settings.api_url, "https://api.bitwarden.eu");
    }

    #[test]
    fn self_hosted_gets_api_and_identity() {
        let settings = client_settings(Some("https://vault.example.com")).unwrap();
        assert_eq!(settings.identity_url, "https://vault.example.com/identity");
        assert_eq!(settings.api_url, "https://vault.example.com/api");
    }

    #[test]
    fn self_hosted_accepts_an_endpoint_url() {
        let settings = client_settings(Some("https://vault.example.com/api")).unwrap();
        assert_eq!(settings.identity_url, "https://vault.example.com/identity");
        assert_eq!(settings.api_url, "https://vault.example.com/api");
    }

    #[test]
    fn self_hosted_keeps_a_path_prefix() {
        let settings = client_settings(Some("https://example.com/bitwarden/")).unwrap();
        assert_eq!(
            settings.identity_url,
            "https://example.com/bitwarden/identity"
        );
        assert_eq!(settings.api_url, "https://example.com/bitwarden/api");
    }

    #[test]
    fn a_host_without_a_scheme_means_https() {
        let settings = client_settings(Some("vault.example.com")).unwrap();
        assert_eq!(settings.api_url, "https://vault.example.com/api");
    }

    #[test]
    fn plain_http_is_refused() {
        assert_eq!(
            client_settings(Some("http://vault.example.com")).unwrap_err(),
            CoreError::InsecureUrl
        );
    }

    #[test]
    fn a_broken_url_is_refused() {
        assert_eq!(
            client_settings(Some("https://")).unwrap_err(),
            CoreError::InvalidUrl
        );
        assert_eq!(
            client_settings(Some("ftp://vault.example.com")).unwrap_err(),
            CoreError::InvalidUrl
        );
        assert_eq!(
            client_settings(Some("https://vault.example.com?x=1")).unwrap_err(),
            CoreError::InvalidUrl
        );
    }
}
