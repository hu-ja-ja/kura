//! Persist the ciphers from a sync response.
//!
//! This SDK revision registers handlers for folders, sends and policies, but
//! not for ciphers. Without one, every sync would leave the vault list empty.

use std::sync::Arc;

use bitwarden_api_api::models::SyncResponseModel;
use bitwarden_core::Client;
use bitwarden_state::registry::StateRegistryError;
use bitwarden_state::repository::Repository;
use bitwarden_sync::{SyncHandler, SyncHandlerError};
use bitwarden_vault::{Cipher, CipherId};

pub struct CipherSyncHandler {
    repository: Arc<dyn Repository<Cipher>>,
}

impl CipherSyncHandler {
    pub fn new(client: &Client) -> Result<Self, StateRegistryError> {
        Ok(Self {
            repository: client.platform().state().get::<Cipher>()?,
        })
    }
}

#[async_trait::async_trait]
impl SyncHandler for CipherSyncHandler {
    async fn on_sync(&self, response: &SyncResponseModel) -> Result<(), SyncHandlerError> {
        let Some(models) = response.ciphers.as_ref() else {
            return Ok(());
        };

        let ciphers: Vec<(CipherId, Cipher)> = models
            .iter()
            .filter_map(|model| {
                let cipher = Cipher::try_from(model.clone()).ok()?;
                let id = cipher.id?;
                Some((id, cipher))
            })
            .collect();

        self.repository.replace_all(ciphers).await?;
        Ok(())
    }
}
