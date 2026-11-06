//! Server URL handling.
//!
//! The SDK wants the identity and the API endpoint separately, so a base URL
//! has to be expanded. Plain HTTP is refused: the SDK only enforces HTTPS in
//! release builds, and the requirement is to refuse it everywhere.

use bitwarden_core::{ClientSettings, DeviceType};
use url::Url;

use crate::error::CoreError;

const USER_AGENT: &str = concat!("Kura/", env!("CARGO_PKG_VERSION"));

/// The base URL a sign-in talks to.
struct Resolved {
    settings: ClientSettings,
    /// The scheme, host, port and path that receive the master password.
    base: String,
    /// The host shown on the login screen, including the official servers.
    display: String,
    /// Whether this is one of the servers the SDK ships with.
    official: bool,
}

impl Resolved {
    /// One of the servers the SDK ships with, where no confirmation is needed.
    fn official(settings: ClientSettings, display: &str) -> Self {
        Self {
            settings,
            base: String::new(),
            display: display.to_string(),
            official: true,
        }
    }
}

/// Build the SDK settings for `server_url`. An empty URL means bitwarden.com.
pub fn client_settings(server_url: Option<&str>) -> Result<ClientSettings, CoreError> {
    Ok(resolve(server_url)?.settings)
}

/// The base URL a sign-in would send the master password to.
///
/// `None` is one of the servers the SDK ships with, which is where the user
/// expects the password to go. `Some` names a server the user typed, so the
/// caller can ask before the password leaves the machine. The path is part of
/// the value: two paths on one host are different servers.
pub fn sign_in_base(server_url: Option<&str>) -> Result<Option<String>, CoreError> {
    let resolved = resolve(server_url)?;
    Ok((!resolved.official).then_some(resolved.base))
}

/// The host a sign-in would use, for the login screen.
///
/// Unlike `sign_in_base`, this also names the servers the SDK ships with, so
/// the target is visible even when no server was configured.
pub fn sign_in_display(server_url: Option<&str>) -> Result<String, CoreError> {
    Ok(resolve(server_url)?.display)
}

/// The server URL that produced a saved session's endpoints.
///
/// A saved session keeps the endpoints instead of the URL the user typed. When
/// the settings have no server, this puts the sign-in back on the server the
/// saved login belongs to instead of bitwarden.com.
pub fn server_url_from_saved(identity_url: &str, api_url: &str) -> Option<String> {
    match (identity_url, api_url) {
        ("https://identity.bitwarden.com", "https://api.bitwarden.com") => return None,
        ("https://identity.bitwarden.eu", "https://api.bitwarden.eu") => {
            return Some("https://vault.bitwarden.eu".to_string());
        }
        _ => {}
    }

    let base = identity_url.strip_suffix("/identity")?;
    if api_url != format!("{base}/api") {
        return None;
    }
    let url = Url::parse(base).ok()?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return None;
    }
    Some(base.to_string())
}

/// Rebuild the settings from the endpoints of a saved session. The URLs come
/// from a file Kura wrote, so they are checked again rather than trusted.
pub fn settings_from_urls(identity_url: &str, api_url: &str) -> Result<ClientSettings, CoreError> {
    for url in [identity_url, api_url] {
        match Url::parse(url) {
            Ok(url) if url.scheme() == "https" => {}
            Ok(_) => return Err(CoreError::InsecureUrl),
            Err(_) => return Err(CoreError::InvalidUrl),
        }
    }

    Ok(ClientSettings {
        identity_url: identity_url.to_string(),
        api_url: api_url.to_string(),
        ..base_settings()
    })
}

fn resolve(server_url: Option<&str>) -> Result<Resolved, CoreError> {
    let mut settings = base_settings();

    let raw = server_url.unwrap_or_default().trim();
    if raw.is_empty() {
        return Ok(Resolved::official(settings, "bitwarden.com"));
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
        "bitwarden.com" | "www.bitwarden.com" | "vault.bitwarden.com" => {
            return Ok(Resolved::official(settings, "bitwarden.com"));
        }
        "bitwarden.eu" | "www.bitwarden.eu" | "vault.bitwarden.eu" => {
            settings.identity_url = "https://identity.bitwarden.eu".to_string();
            settings.api_url = "https://api.bitwarden.eu".to_string();
            return Ok(Resolved::official(settings, "bitwarden.eu"));
        }
        _ => {}
    }

    let mut host = host;
    if let Some(port) = url.port() {
        host.push_str(&format!(":{port}"));
    }
    let mut base = format!("{}://{}", url.scheme(), host);
    let path = url.path().trim_end_matches('/');
    if !path.is_empty() {
        base.push_str(path);
    }
    settings.identity_url = format!("{base}/identity");
    settings.api_url = format!("{base}/api");

    Ok(Resolved {
        settings,
        base,
        display: host,
        official: false,
    })
}

fn base_settings() -> ClientSettings {
    ClientSettings {
        user_agent: USER_AGENT.to_string(),
        // The server rejects requests without this header since 2026.
        bitwarden_client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        device_type: DeviceType::WindowsDesktop,
        ..Default::default()
    }
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

    #[test]
    fn the_official_servers_need_no_confirmation() {
        assert_eq!(sign_in_base(None).unwrap(), None);
        assert_eq!(sign_in_base(Some("")).unwrap(), None);
        assert_eq!(sign_in_base(Some("vault.bitwarden.com")).unwrap(), None);
        assert_eq!(sign_in_base(Some("https://bitwarden.eu")).unwrap(), None);
    }

    #[test]
    fn a_self_hosted_server_names_its_base_url() {
        assert_eq!(
            sign_in_base(Some("https://vault.example.com")).unwrap(),
            Some("https://vault.example.com".to_string())
        );
        assert_eq!(
            sign_in_base(Some("vault.example.com/api")).unwrap(),
            Some("https://vault.example.com".to_string())
        );
    }

    #[test]
    fn a_named_port_stays_part_of_the_base_url() {
        assert_eq!(
            sign_in_base(Some("https://vault.example.com:8443")).unwrap(),
            Some("https://vault.example.com:8443".to_string())
        );
    }

    #[test]
    fn the_identity_endpoint_is_not_mistaken_for_a_default_server() {
        // `client_settings` would turn this into a self-hosted endpoint, so the
        // confirmation has to follow the input host, not the resulting URL.
        assert_eq!(
            sign_in_base(Some("https://identity.bitwarden.com")).unwrap(),
            Some("https://identity.bitwarden.com".to_string())
        );
    }

    #[test]
    fn a_lookalike_host_is_not_the_default_server() {
        assert_eq!(
            sign_in_base(Some("https://vault.bitwarden.com.evil.example")).unwrap(),
            Some("https://vault.bitwarden.com.evil.example".to_string())
        );
    }

    #[test]
    fn credentials_in_the_url_do_not_rename_the_host() {
        assert_eq!(
            sign_in_base(Some("https://bitwarden.com@evil.example")).unwrap(),
            Some("https://evil.example".to_string())
        );
    }

    #[test]
    fn a_path_is_part_of_the_base_url() {
        assert_eq!(
            sign_in_base(Some("https://example.com/bitwarden")).unwrap(),
            Some("https://example.com/bitwarden".to_string())
        );
        assert_eq!(
            sign_in_base(Some("https://example.com/bitwarden/identity")).unwrap(),
            Some("https://example.com/bitwarden".to_string())
        );
        assert_ne!(
            sign_in_base(Some("https://example.com/bitwarden")).unwrap(),
            sign_in_base(Some("https://example.com/other")).unwrap()
        );
    }

    #[test]
    fn the_login_screen_names_the_server_even_without_a_setting() {
        assert_eq!(sign_in_display(None).unwrap(), "bitwarden.com");
        assert_eq!(sign_in_display(Some("   ")).unwrap(), "bitwarden.com");
        assert_eq!(
            sign_in_display(Some("https://vault.bitwarden.eu")).unwrap(),
            "bitwarden.eu"
        );
    }

    #[test]
    fn the_login_screen_names_a_custom_host_without_the_path() {
        assert_eq!(
            sign_in_display(Some("https://vault.example.com:8443/bitwarden")).unwrap(),
            "vault.example.com:8443"
        );
    }

    #[test]
    fn a_saved_server_comes_back_from_its_endpoints() {
        assert_eq!(
            server_url_from_saved(
                "https://identity.bitwarden.com",
                "https://api.bitwarden.com"
            ),
            None
        );
        assert_eq!(
            server_url_from_saved("https://identity.bitwarden.eu", "https://api.bitwarden.eu")
                .as_deref(),
            Some("https://vault.bitwarden.eu")
        );
        assert_eq!(
            server_url_from_saved(
                "https://vault.example.com:8443/bitwarden/identity",
                "https://vault.example.com:8443/bitwarden/api"
            )
            .as_deref(),
            Some("https://vault.example.com:8443/bitwarden")
        );
    }

    #[test]
    fn saved_endpoints_that_do_not_belong_together_name_no_server() {
        assert_eq!(
            server_url_from_saved(
                "https://vault.example.com/identity",
                "https://other.example/api"
            ),
            None
        );
        assert_eq!(
            server_url_from_saved(
                "http://vault.example.com/identity",
                "http://vault.example.com/api"
            ),
            None
        );
        assert_eq!(
            server_url_from_saved("https://vault.example.com", "https://vault.example.com/api"),
            None
        );
    }
}
