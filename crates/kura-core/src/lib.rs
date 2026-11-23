//! UI-independent vault engine. Keep the SDK behind this crate so the UI can be
//! swapped without touching vault logic.

mod cipher_sync;
mod error;
mod generator;
mod model;
mod search;
mod server;
mod vault;
mod write;

pub use error::CoreError;
pub use generator::{
    GeneratorError, GeneratorMode, GeneratorOptions, MAX_MIN_CHARS, MAX_PASSPHRASE_WORDS,
    MAX_PASSWORD_LENGTH, MIN_PASSPHRASE_WORDS, MIN_PASSWORD_LENGTH, PassphraseOptions,
    PasswordOptions, effective_password_length, generate, minimum_password_length,
};
pub use model::{
    AttachmentInfo, CardDetail, CustomField, Folder, IdentityDetail, ItemKind, SshKeyDetail,
    TotpCode, VaultItem, VaultItemDetail, VaultSnapshot, openable_url,
};
pub use search::{VaultFilter, filter_indices};
pub use server::{client_settings, server_url_from_saved, sign_in_base, sign_in_display};
pub use vault::{LoginOutcome, SavedSession, TwoFactorChallenge, TwoFactorMethod, Vault};
pub use write::{
    LoginDraft, LoginEditor, LoginFieldDraft, LoginFieldKind, LoginUriDraft, UriMatch,
};
