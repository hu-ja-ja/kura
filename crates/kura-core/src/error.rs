//! Error type shared with the UI.
//!
//! The variants carry no localized text: the app maps each one to a message.
//! `Rejected` keeps the server's own wording, which is shown as it is.
//! `Internal` carries a static, non-secret label for the log; it is not shown.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// The email or the master password was rejected.
    InvalidCredentials,
    /// The two-factor code was rejected.
    TwoFactorFailed,
    /// The account needs a two-factor method Kura does not implement.
    TwoFactorUnsupported,
    /// The server refused the sign-in. Carries the reason it gave.
    Rejected(String),
    /// The sign-in passed but the vault key was not loaded.
    KeyNotLoaded,
    /// The server could not be reached.
    Network,
    /// The server did not answer in time.
    Timeout,
    /// The server answered with something Kura cannot read.
    InvalidResponse,
    /// The item is no longer in the vault.
    NotFound,
    /// The item has no TOTP key.
    NoTotp,
    /// The item has no password field.
    NoPassword,
    /// The server refused the request. Carries the server's message.
    Server(String),
    /// The account no longer exists on the server.
    AccountDeleted,
    /// The server URL cannot be used.
    InvalidUrl,
    /// The server URL is plain HTTP. Rejected on purpose.
    InsecureUrl,
    /// Anything else. The label names the failing step for the log and never
    /// contains a vault value or server text.
    Internal(&'static str),
}

impl CoreError {
    /// A stable name for the log. Never includes server text or vault data.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidCredentials => "invalid_credentials",
            Self::TwoFactorFailed => "two_factor_failed",
            Self::TwoFactorUnsupported => "two_factor_unsupported",
            Self::Rejected(_) => "rejected",
            Self::KeyNotLoaded => "key_not_loaded",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid_response",
            Self::NotFound => "not_found",
            Self::NoTotp => "no_totp",
            Self::NoPassword => "no_password",
            Self::Server(_) => "server",
            Self::AccountDeleted => "account_deleted",
            Self::InvalidUrl => "invalid_url",
            Self::InsecureUrl => "insecure_url",
            Self::Internal(_) => "internal",
        }
    }

    /// The static step label of an [`CoreError::Internal`], for the log.
    pub fn context(&self) -> Option<&'static str> {
        match self {
            Self::Internal(context) => Some(context),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_variant() -> Vec<CoreError> {
        vec![
            CoreError::InvalidCredentials,
            CoreError::TwoFactorFailed,
            CoreError::TwoFactorUnsupported,
            CoreError::Rejected("server text".to_string()),
            CoreError::KeyNotLoaded,
            CoreError::Network,
            CoreError::Timeout,
            CoreError::InvalidResponse,
            CoreError::NotFound,
            CoreError::NoTotp,
            CoreError::NoPassword,
            CoreError::Server("server text".to_string()),
            CoreError::AccountDeleted,
            CoreError::InvalidUrl,
            CoreError::InsecureUrl,
            CoreError::Internal("a static step"),
        ]
    }

    #[test]
    fn every_code_is_a_unique_snake_case_name() {
        let mut seen = Vec::new();
        for error in every_variant() {
            let code = error.code();
            assert!(
                code.chars()
                    .all(|character| character.is_ascii_lowercase() || character == '_'),
                "{code} is not a snake_case name"
            );
            assert!(!seen.contains(&code), "duplicate code {code}");
            seen.push(code);
        }
    }

    #[test]
    fn server_text_stays_out_of_the_code_and_context() {
        let rejected = CoreError::Rejected("a secret-looking body".to_string());
        let server = CoreError::Server("a secret-looking body".to_string());

        assert_eq!(rejected.code(), "rejected");
        assert_eq!(server.code(), "server");
        assert_eq!(rejected.context(), None);
        assert_eq!(server.context(), None);
    }

    #[test]
    fn an_internal_label_is_kept_for_the_log() {
        let error = CoreError::Internal("read the saved tokens");

        assert_eq!(error.code(), "internal");
        assert_eq!(error.context(), Some("read the saved tokens"));
    }
}
