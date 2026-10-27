//! The vault engine.
//!
//! Wraps the Bitwarden SDK so the UI never sees SDK types and never blocks on
//! the network.

use std::sync::Arc;
use std::time::Duration;

use bitwarden_core::auth::login::PreloginError;
use bitwarden_core::auth::login::response::two_factor::TwoFactorProviders;
use bitwarden_core::auth::login::{
    LoginError, PasswordLoginRequest, PasswordLoginResponse, TwoFactorEmailError,
    TwoFactorEmailRequest, TwoFactorProvider, TwoFactorRequest,
};
use bitwarden_core::{ApiError, FromClient};
use bitwarden_pm::PasswordManagerClient;
use bitwarden_sync::{SyncClient, SyncError, SyncRequest};
use bitwarden_vault::{CipherId, CipherView, FolderSyncHandler, GetCipherError};
use tokio::time::timeout;

use crate::cipher_sync::CipherSyncHandler;
use crate::error::CoreError;
use crate::model::{TotpCode, VaultItemDetail, VaultSnapshot};
use crate::server::client_settings;

const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);

/// Which step of the login produced an error. The same server answer means
/// different things in each step.
#[derive(Clone, Copy)]
enum Step {
    Initial,
    TwoFactor,
}

/// The result of a password login.
pub enum LoginOutcome {
    /// The vault is open.
    LoggedIn,
    /// A second factor is needed before the vault opens.
    TwoFactorRequired(TwoFactorChallenge),
}

/// The second factors the account accepts.
#[derive(Clone)]
pub struct TwoFactorChallenge {
    pub methods: Vec<TwoFactorMethod>,
}

#[derive(Clone)]
pub enum TwoFactorMethod {
    /// A code from an authenticator app.
    Authenticator,
    /// A code sent to this address.
    Email { address: String },
}

pub struct Vault {
    client: PasswordManagerClient,
    sync: SyncClient,
}

impl Vault {
    /// Open a client for `server_url`. An empty URL means bitwarden.com.
    pub fn connect(server_url: Option<&str>) -> Result<Self, CoreError> {
        let settings = client_settings(server_url)?;
        let client = PasswordManagerClient::new(Some(settings));

        // `PasswordManagerClient::new_with_sync` registers its handlers on a
        // `SyncClient` that is dropped at the end of that call, and every
        // `client.sync()` builds a fresh one with an empty registry. Keep one
        // `SyncClient` and register the handlers here. Ciphers have no handler
        // in this SDK revision, so CipherSyncHandler supplies it.
        //
        // The crypto sync handler is left out on purpose: it needs a host
        // StateBridge, which a native Rust host does not have in this revision.
        // The client is already unlocked by the login, so the vault works
        // without it. Server-side key rotations are applied on the next login.
        let sync = client.sync();
        sync.register_sync_handler(Arc::new(FolderSyncHandler::from_client(&client.0)));
        sync.register_sync_handler(Arc::new(
            CipherSyncHandler::new(&client.0).map_err(|_| CoreError::Internal)?,
        ));

        Ok(Self { client, sync })
    }

    pub fn is_unlocked(&self) -> bool {
        self.client.is_unlocked()
    }

    /// Log in with the master password. The account may ask for a second factor.
    pub async fn login(&self, email: &str, password: &str) -> Result<LoginOutcome, CoreError> {
        let response = timeout(
            LOGIN_TIMEOUT,
            self.client.0.auth().login_password(&PasswordLoginRequest {
                email: email.to_string(),
                password: password.to_string(),
                two_factor: None,
            }),
        )
        .await
        .map_err(|_| CoreError::Timeout)?
        .map_err(|err| map_login_error(err, Step::Initial))?;

        self.read_login_response(response)
    }

    /// Answer the second factor challenge.
    pub async fn submit_two_factor(
        &self,
        email: &str,
        password: &str,
        method: &TwoFactorMethod,
        code: &str,
    ) -> Result<(), CoreError> {
        let provider = match method {
            TwoFactorMethod::Authenticator => TwoFactorProvider::Authenticator,
            TwoFactorMethod::Email { .. } => TwoFactorProvider::Email,
        };

        let response = timeout(
            LOGIN_TIMEOUT,
            self.client.0.auth().login_password(&PasswordLoginRequest {
                email: email.to_string(),
                password: password.to_string(),
                two_factor: Some(TwoFactorRequest {
                    token: code.trim().to_string(),
                    provider,
                    remember: false,
                }),
            }),
        )
        .await
        .map_err(|_| CoreError::Timeout)?
        .map_err(|err| map_login_error(err, Step::TwoFactor))?;

        match self.read_login_response(response)? {
            LoginOutcome::LoggedIn => Ok(()),
            LoginOutcome::TwoFactorRequired(_) => Err(CoreError::TwoFactorFailed),
        }
    }

    /// Ask the server to send a code to the account's address.
    pub async fn send_email_code(&self, email: &str, password: &str) -> Result<(), CoreError> {
        timeout(
            LOGIN_TIMEOUT,
            self.client
                .0
                .auth()
                .send_two_factor_email(&TwoFactorEmailRequest {
                    password: password.to_string(),
                    email: email.to_string(),
                }),
        )
        .await
        .map_err(|_| CoreError::Timeout)?
        .map_err(map_email_error)
    }

    /// Pull the vault from the server. `force` skips the revision date check.
    pub async fn sync(&self, force: bool) -> Result<(), CoreError> {
        timeout(
            SYNC_TIMEOUT,
            self.sync.sync(SyncRequest {
                force,
                exclude_subdomains: None,
            }),
        )
        .await
        .map_err(|_| CoreError::Timeout)?
        .map(|_| ())
        .map_err(map_sync_error)
    }

    /// The current list of items, decrypted and ready to show.
    pub async fn snapshot(&self) -> Result<VaultSnapshot, CoreError> {
        let decrypted = self
            .client
            .vault()
            .ciphers()
            .get_all()
            .await
            .map_err(|_| CoreError::Internal)?;

        let mut items: Vec<_> = decrypted
            .successes
            .into_iter()
            .filter(|cipher| cipher.deleted_date.is_none())
            .map(crate::model::VaultItem::from)
            .collect();
        items.sort_by_cached_key(|item| item.name.to_lowercase());

        // Folders only feed the filter bar, so a failure must not hide the list.
        let mut folders: Vec<crate::model::Folder> = self
            .client
            .vault()
            .folders()
            .list()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|folder| {
                Some(crate::model::Folder {
                    id: folder.id?.to_string(),
                    name: folder.name,
                })
            })
            .collect();
        folders.sort_by_cached_key(|folder| folder.name.to_lowercase());

        Ok(VaultSnapshot {
            items,
            folders,
            decrypt_failures: decrypted.failures.len(),
        })
    }

    /// Decrypt one item for the detail pane.
    pub async fn detail(&self, id: &str) -> Result<VaultItemDetail, CoreError> {
        Ok(VaultItemDetail::from(self.cipher_view(id).await?))
    }

    /// Generate the current TOTP for one item, without handing out the seed.
    pub async fn totp(&self, id: &str) -> Result<TotpCode, CoreError> {
        let view = self.cipher_view(id).await?;
        crate::model::totp_for_view(&view).ok_or(CoreError::NoTotp)
    }

    /// The password of one login, for the list's copy button.
    pub async fn password(&self, id: &str) -> Result<String, CoreError> {
        let view = self.cipher_view(id).await?;
        view.login
            .and_then(|login| login.password)
            .filter(|password| !password.trim().is_empty())
            .ok_or(CoreError::NoPassword)
    }

    async fn cipher_view(&self, id: &str) -> Result<CipherView, CoreError> {
        let id: CipherId = id.parse().map_err(|_| CoreError::NotFound)?;
        self.client
            .vault()
            .ciphers()
            .get(id)
            .await
            .map_err(map_get_error)
    }

    fn read_login_response(
        &self,
        response: PasswordLoginResponse,
    ) -> Result<LoginOutcome, CoreError> {
        if response.authenticated {
            // Authenticated but without the user key means every later call
            // would fail with a less useful error.
            if !self.is_unlocked() {
                return Err(CoreError::KeyNotLoaded);
            }
            return Ok(LoginOutcome::LoggedIn);
        }

        let providers = response.two_factor.ok_or(CoreError::InvalidResponse)?;
        let methods = supported_methods(providers);
        if methods.is_empty() {
            return Err(CoreError::TwoFactorUnsupported);
        }

        Ok(LoginOutcome::TwoFactorRequired(TwoFactorChallenge {
            methods,
        }))
    }
}

/// The second factors this build can answer.
fn supported_methods(providers: TwoFactorProviders) -> Vec<TwoFactorMethod> {
    let mut methods = Vec::new();
    if providers.authenticator.is_some() {
        methods.push(TwoFactorMethod::Authenticator);
    }
    if let Some(email) = providers.email {
        methods.push(TwoFactorMethod::Email {
            address: email.email,
        });
    }
    methods
}

fn map_login_error(err: LoginError, step: Step) -> CoreError {
    match err {
        LoginError::IdentityFail(reason) => {
            let detail = reason.to_string();
            if detail.trim().is_empty() {
                match step {
                    Step::Initial => CoreError::InvalidCredentials,
                    Step::TwoFactor => CoreError::TwoFactorFailed,
                }
            } else {
                CoreError::Rejected(detail)
            }
        }
        LoginError::AuthenticationFailed => match step {
            Step::Initial => CoreError::InvalidCredentials,
            Step::TwoFactor => CoreError::TwoFactorFailed,
        },
        LoginError::Api(api) => map_api_error(&api),
        LoginError::Prelogin(PreloginError::Api(api)) => map_api_error(&api),
        LoginError::Prelogin(PreloginError::MissingField(_)) => CoreError::InvalidResponse,
        LoginError::InvalidResponse | LoginError::Serde(_) => CoreError::InvalidResponse,
        _ => CoreError::Internal,
    }
}

fn map_email_error(err: TwoFactorEmailError) -> CoreError {
    match err {
        TwoFactorEmailError::Api(api) => map_api_error(&api),
        TwoFactorEmailError::Prelogin(PreloginError::Api(api)) => map_api_error(&api),
        TwoFactorEmailError::Prelogin(PreloginError::MissingField(_)) => CoreError::InvalidResponse,
        TwoFactorEmailError::Crypto(_) => CoreError::Internal,
    }
}

fn map_get_error(err: GetCipherError) -> CoreError {
    match err {
        GetCipherError::ItemNotFound(_) => CoreError::NotFound,
        GetCipherError::Crypto(_) | GetCipherError::Repository(_) => CoreError::Internal,
    }
}

fn map_sync_error(err: SyncError) -> CoreError {
    match err {
        SyncError::Api(api) => map_api_error(&api),
        SyncError::AccountDeleted => {
            CoreError::Server("The account has been deleted on the server.".to_string())
        }
        SyncError::InvalidRevisionDate => CoreError::InvalidResponse,
        _ => CoreError::Internal,
    }
}

fn map_api_error(err: &ApiError) -> CoreError {
    match err {
        ApiError::Reqwest(err) => {
            if err.is_timeout() {
                CoreError::Timeout
            } else {
                CoreError::Network
            }
        }
        ApiError::ReqwestMiddleware(_) | ApiError::Io(_) => CoreError::Network,
        ApiError::Serde(_) => CoreError::InvalidResponse,
        ApiError::Response(content) => match content.status.as_u16() {
            400 | 401 | 403 => CoreError::Rejected(response_detail(&content.message)),
            _ => CoreError::Server(content.message.clone()),
        },
    }
}

/// Pull the useful part out of an identity error body so the UI can show why
/// the server said no. Falls back to the body itself.
fn response_detail(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        for pointer in [
            "/error_model/message",
            "/ErrorModel/Message",
            "/error_description",
            "/error",
        ] {
            if let Some(text) = value
                .pointer(pointer)
                .and_then(|value| value.as_str())
                .filter(|text| !text.trim().is_empty())
            {
                return text.to_string();
            }
        }
    }

    const MAX: usize = 400;
    if body.chars().count() <= MAX {
        body.to_string()
    } else {
        body.chars().take(MAX).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_constructs() {
        let vault = Vault::connect(None).unwrap();
        assert!(!vault.is_unlocked());
    }

    #[tokio::test]
    async fn a_malformed_item_id_is_not_found() {
        let vault = Vault::connect(None).unwrap();
        assert_eq!(
            vault.detail("not-a-uuid").await.err(),
            Some(CoreError::NotFound)
        );
        assert_eq!(
            vault.totp("not-a-uuid").await.err(),
            Some(CoreError::NotFound)
        );
        assert_eq!(
            vault.password("not-a-uuid").await.err(),
            Some(CoreError::NotFound)
        );
    }

    #[test]
    fn a_bad_url_is_refused_before_any_request() {
        assert_eq!(
            Vault::connect(Some("http://vault.example.com")).err(),
            Some(CoreError::InsecureUrl)
        );
    }
}
