//! The shape of the vault the UI sees.
//!
//! No secrets live here: passwords, TOTP seeds and notes stay inside the SDK.

use bitwarden_vault::{CipherType, CipherView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Login,
    Card,
    Identity,
    SecureNote,
    SshKey,
    /// A kind this build does not know about yet.
    Other,
}

impl From<CipherType> for ItemKind {
    fn from(kind: CipherType) -> Self {
        match kind {
            CipherType::Login => ItemKind::Login,
            CipherType::Card => ItemKind::Card,
            CipherType::Identity => ItemKind::Identity,
            CipherType::SecureNote => ItemKind::SecureNote,
            CipherType::SshKey => ItemKind::SshKey,
            _ => ItemKind::Other,
        }
    }
}

/// One row of the vault list.
#[derive(Clone)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    /// Secondary line: the user name, or the first URI when there is none.
    pub subtitle: String,
    /// Kept for search. Not a secret on its own, but treated like one.
    pub username: String,
    /// Kept for search.
    pub uris: Vec<String>,
    pub kind: ItemKind,
    pub favorite: bool,
}

impl From<CipherView> for VaultItem {
    fn from(view: CipherView) -> Self {
        let (username, uris) = match view.login {
            Some(ref login) => (
                login.username.clone().unwrap_or_default(),
                login
                    .uris
                    .as_ref()
                    .map(|uris| uris.iter().filter_map(|uri| uri.uri.clone()).collect())
                    .unwrap_or_default(),
            ),
            None => (String::new(), Vec::new()),
        };

        let subtitle = if username.is_empty() {
            uris.first().cloned().unwrap_or_default()
        } else {
            username.clone()
        };

        Self {
            id: view.id.map(|id| id.to_string()).unwrap_or_default(),
            name: view.name,
            subtitle,
            username,
            uris,
            kind: ItemKind::from(view.r#type),
            favorite: view.favorite,
        }
    }
}

/// The whole list, as of the last sync.
#[derive(Clone, Default)]
pub struct VaultSnapshot {
    pub items: Vec<VaultItem>,
    /// Items the SDK could not decrypt. They are not shown.
    pub decrypt_failures: usize,
}
