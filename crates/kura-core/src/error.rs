//! Error type shared with the UI.
//!
//! The variants carry no localized text: the app maps each one to a message.
//! `Rejected` keeps the server's own wording, which is shown as it is.

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
    /// The server refused the request. Carries the server's message.
    Server(String),
    /// The server URL cannot be used.
    InvalidUrl,
    /// The server URL is plain HTTP. Rejected on purpose.
    InsecureUrl,
    /// Anything else.
    Internal,
}
