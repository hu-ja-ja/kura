//! UI-independent vault engine. Keep the SDK behind this crate so the UI can be
//! swapped without touching vault logic.

use anyhow::{Context, Result};
use bitwarden_core::auth::login::{PasswordLoginRequest, PasswordLoginResponse};
use bitwarden_pm::PasswordManagerClient;
use bitwarden_sync::SyncRequest;
use bitwarden_vault::CipherListView;

pub struct Vault {
    client: PasswordManagerClient,
}

/// The only shape the UI sees. Deliberately small: no passwords, no SDK types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub favorite: bool,
}

impl Vault {
    pub fn new() -> Self {
        Self {
            client: PasswordManagerClient::new_with_sync(None),
        }
    }

    pub fn is_unlocked(&self) -> bool {
        self.client.is_unlocked()
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<PasswordLoginResponse> {
        self.client
            .0
            .auth()
            .login_password(&PasswordLoginRequest {
                email: email.to_string(),
                password: password.to_string(),
                two_factor: None,
            })
            .await
            .context("login failed")
    }

    pub async fn sync(&self) -> Result<()> {
        self.client
            .sync()
            .sync(SyncRequest {
                force: false,
                exclude_subdomains: None,
            })
            .await
            .context("sync failed")?;
        Ok(())
    }

    /// Summary rows only. Full item data stays in the SDK until requested.
    pub async fn list(&self) -> Result<Vec<VaultItem>> {
        let decrypted = self
            .client
            .vault()
            .ciphers()
            .list()
            .await
            .context("listing ciphers failed")?;
        Ok(decrypted.successes.into_iter().map(VaultItem::from).collect())
    }
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl From<CipherListView> for VaultItem {
    fn from(view: CipherListView) -> Self {
        Self {
            id: view.id.map(|id| id.to_string()).unwrap_or_default(),
            name: view.name,
            subtitle: view.subtitle,
            favorite: view.favorite,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_constructs() {
        let vault = Vault::new();
        assert!(!vault.is_unlocked());
    }
}
