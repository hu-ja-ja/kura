//! UI-independent vault engine. Keep the SDK behind this crate so the UI can be
//! swapped without touching vault logic.

mod cipher_sync;
mod error;
mod model;
mod search;
mod server;
mod vault;

pub use error::CoreError;
pub use model::{
    AttachmentInfo, CardDetail, CustomField, Folder, IdentityDetail, ItemKind, SshKeyDetail,
    TotpCode, VaultItem, VaultItemDetail, VaultSnapshot, openable_url,
};
pub use search::{VaultFilter, filter_indices, search_indices};
pub use server::client_settings;
pub use vault::{LoginOutcome, TwoFactorChallenge, TwoFactorMethod, Vault};
