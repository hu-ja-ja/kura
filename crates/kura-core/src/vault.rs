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
use bitwarden_core::client::login_method::UserLoginMethod;
use bitwarden_core::client::persisted_state::{
    AUTHENTICATION_TOKENS, AuthenticationTokens, USER_LOGIN_METHOD,
};
use bitwarden_core::key_management::account_cryptographic_state::WrappedAccountCryptographicState;
use bitwarden_core::key_management::crypto::{InitUserCryptoMethod, InitUserCryptoRequest};
use bitwarden_core::key_management::{
    MasterPasswordUnlockData, PrivateKeySlotId, SymmetricKeySlotId,
};
use bitwarden_core::{ApiError, ClientSettings, FromClient, UserId};
use bitwarden_crypto::{MasterKey, SymmetricCryptoKey};
use bitwarden_pm::PasswordManagerClient;
use bitwarden_sync::{SyncClient, SyncError, SyncRequest};
use bitwarden_vault::{CipherId, CipherView, FolderSyncHandler, GetCipherError};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::cipher_sync::CipherSyncHandler;
use crate::error::CoreError;
use crate::model::{TotpCode, VaultItemDetail, VaultSnapshot};
use crate::server::{client_settings, settings_from_urls};

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
    identity_url: String,
    api_url: String,
}

/// Everything needed to open the vault again with only the master password.
///
/// The tokens are bearer credentials, so the app keeps this on disk encrypted.
/// The keys inside stay wrapped by the master key: the file alone opens nothing.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedSession {
    /// The address the user signed in with. Not the KDF salt, which the server may set.
    pub email: String,
    pub user_id: UserId,
    pub identity_url: String,
    pub api_url: String,
    pub tokens: AuthenticationTokens,
    pub crypto_state: WrappedAccountCryptographicState,
    pub unlock: MasterPasswordUnlockData,
}

impl SavedSession {
    /// Whether this session belongs to the account the user just typed.
    pub fn matches(&self, server_url: &str, email: &str) -> bool {
        self.email.eq_ignore_ascii_case(email.trim())
            && client_settings(Some(server_url)).is_ok_and(|settings| {
                settings.identity_url == self.identity_url && settings.api_url == self.api_url
            })
    }
}

impl Vault {
    /// Open a client for `server_url`. An empty URL means bitwarden.com.
    pub fn connect(server_url: Option<&str>) -> Result<Self, CoreError> {
        Self::from_settings(client_settings(server_url)?)
    }

    fn from_settings(settings: ClientSettings) -> Result<Self, CoreError> {
        let identity_url = settings.identity_url.clone();
        let api_url = settings.api_url.clone();
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
        //
        // shortcut: the SDK builds its identity client without a redirect
        // policy, so a 307 or 308 from a self-hosted identity endpoint would
        // resend the master password hash to the redirect target. The API
        // client is safe because bitwarden-pm always supplies middleware, which
        // selects `Policy::none`. Upgrade when the SDK exposes a redirect
        // policy for the identity client.
        let sync = client.sync();
        sync.register_sync_handler(Arc::new(FolderSyncHandler::from_client(&client.0)));
        sync.register_sync_handler(Arc::new(
            CipherSyncHandler::new(&client.0)
                .map_err(|_| CoreError::Internal("register the cipher sync handler"))?,
        ));

        Ok(Self {
            client,
            sync,
            identity_url,
            api_url,
        })
    }

    /// Whether `saved` was made for `email` on this vault's server. A sign-in
    /// to another account must not write its tokens into that file.
    pub fn owns(&self, saved: &SavedSession, email: &str) -> bool {
        saved.email.eq_ignore_ascii_case(email.trim())
            && saved.identity_url == self.identity_url
            && saved.api_url == self.api_url
    }

    /// Open a saved session with the master password. No sign-in, no second factor.
    pub async fn restore(saved: &SavedSession, password: &str) -> Result<Self, CoreError> {
        let vault = Self::from_settings(settings_from_urls(&saved.identity_url, &saved.api_url)?)?;

        vault
            .client
            .0
            .platform()
            .state()
            .setting(AUTHENTICATION_TOKENS)
            .map_err(|_| CoreError::Internal("read the saved tokens"))?
            .update(saved.tokens.clone())
            .await
            .map_err(|_| CoreError::Internal("install the saved tokens"))?;

        vault.unlock(saved, password).await?;

        Ok(vault)
    }

    /// Open the keys of a client that is already connected.
    ///
    /// A lock clears the key store but leaves the encrypted items in the
    /// client, so this is enough to show the old list again before a sync.
    pub async fn unlock(&self, saved: &SavedSession, password: &str) -> Result<(), CoreError> {
        self.client
            .0
            .crypto()
            .initialize_user_crypto(InitUserCryptoRequest {
                user_id: Some(saved.user_id),
                kdf_params: saved.unlock.kdf.clone(),
                email: saved.unlock.salt.clone(),
                account_cryptographic_state: saved.crypto_state.clone(),
                method: InitUserCryptoMethod::MasterPasswordUnlock {
                    password: password.to_string(),
                    master_password_unlock: saved.unlock.clone(),
                },
                upgrade_token: None,
            })
            .await
            .map_err(|_| CoreError::InvalidCredentials)?;

        // `initialize_user_crypto` records a login method with an empty client
        // id. The token middleware copies that id into every refresh request,
        // so put back the one a password sign-in uses.
        self.client
            .0
            .platform()
            .state()
            .setting(USER_LOGIN_METHOD)
            .map_err(|_| CoreError::Internal("read the login method"))?
            .update(UserLoginMethod::Username {
                client_id: "web".to_string(),
                email: saved.unlock.salt.clone(),
                kdf: saved.unlock.kdf.clone(),
            })
            .await
            .map_err(|_| CoreError::Internal("install the login method"))?;

        Ok(())
    }

    /// Drop the keys but keep the client and its encrypted items for the next
    /// unlock. Nothing about the vault reaches the disk.
    pub async fn lock(&self) {
        self.client.unlock().lock().await;
    }

    /// The tokens the SDK currently holds. It renews them during a request,
    /// so this is not always what the saved login was written with.
    async fn tokens(&self) -> Result<AuthenticationTokens, CoreError> {
        self.client
            .0
            .platform()
            .state()
            .setting(AUTHENTICATION_TOKENS)
            .map_err(|_| CoreError::Internal("read the authentication tokens"))?
            .get()
            .await
            .map_err(|_| CoreError::Internal("read the authentication tokens"))?
            .ok_or(CoreError::Internal("the session has no tokens"))
    }

    /// Copy renewed tokens into `saved`. Returns whether anything changed.
    pub async fn update_saved_tokens(&self, saved: &mut SavedSession) -> Result<bool, CoreError> {
        let tokens = self.tokens().await?;
        if saved.tokens.access_token == tokens.access_token
            && saved.tokens.refresh_token == tokens.refresh_token
            && saved.tokens.expires_on == tokens.expires_on
        {
            return Ok(false);
        }
        saved.tokens = tokens;
        Ok(true)
    }

    /// The state that lets the next start skip the sign-in.
    pub async fn saved_session(
        &self,
        email: &str,
        password: &str,
    ) -> Result<SavedSession, CoreError> {
        let state = self.client.0.platform().state();
        let tokens = self.tokens().await?;
        let crypto_state = {
            let key_store = self.client.0.internal.get_key_store();
            let ctx = key_store.context();
            // The sign-in installed this key from the server's answer; wrapping it
            // again with the user key gives the state a restore can install.
            let private_key = ctx
                .wrap_private_key(SymmetricKeySlotId::User, PrivateKeySlotId::UserPrivateKey)
                .map_err(|_| CoreError::Internal("wrap the private key"))?;
            WrappedAccountCryptographicState::V1 { private_key }
        };
        let login_method = state
            .setting(USER_LOGIN_METHOD)
            .map_err(|_| CoreError::Internal("read the login method"))?
            .get()
            .await
            .map_err(|_| CoreError::Internal("read the login method"))?
            .ok_or(CoreError::Internal("the saved login has no login method"))?;
        let UserLoginMethod::Username {
            email: salt, kdf, ..
        } = login_method
        else {
            return Err(CoreError::Internal("the saved login is not password based"));
        };

        let user_key = self
            .client
            .0
            .crypto()
            .get_user_encryption_key()
            .await
            .map_err(|_| CoreError::Internal("read the user key"))?;
        let user_key = SymmetricCryptoKey::try_from(user_key)
            .map_err(|_| CoreError::Internal("convert the user key"))?;
        let master_key = MasterKey::derive(password, &salt, &kdf)
            .map_err(|_| CoreError::Internal("derive the master key"))?;

        Ok(SavedSession {
            email: email.trim().to_string(),
            // A restore needs an id for the SDK's per-user local state, but the
            // sign-in response carries none and nothing else reads it.
            user_id: UserId::new_v4(),
            identity_url: self.identity_url.clone(),
            api_url: self.api_url.clone(),
            tokens,
            crypto_state,
            unlock: MasterPasswordUnlockData {
                kdf,
                salt,
                master_key_wrapped_user_key: master_key
                    .encrypt_user_key(&user_key)
                    .map_err(|_| CoreError::Internal("wrap the user key"))?,
                contained_key_id: None,
            },
        })
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
    /// Returns whether a sync was performed; false means the vault was already
    /// up to date.
    pub async fn sync(&self, force: bool) -> Result<bool, CoreError> {
        timeout(
            SYNC_TIMEOUT,
            self.sync.sync(SyncRequest {
                force,
                // Kura does not match equivalent domains, so do not download them.
                exclude_subdomains: Some(true),
            }),
        )
        .await
        .map_err(|_| CoreError::Timeout)?
        .map_err(map_sync_error)
    }

    /// The current list of items, decrypted and ready to show.
    pub async fn snapshot(&self) -> Result<VaultSnapshot, CoreError> {
        // `list` decrypts only the list fields. That helps items in the old
        // field-level format; a blob-format item is unsealed whole anyway.
        let decrypted = self
            .client
            .vault()
            .ciphers()
            .list()
            .await
            .map_err(|_| CoreError::Internal("read the decrypted items"))?;

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
    pub async fn password(&self, id: &str) -> Result<Zeroizing<String>, CoreError> {
        let view = self.cipher_view(id).await?;
        view.login
            .and_then(|login| login.password)
            .filter(|password| !password.trim().is_empty())
            .map(Zeroizing::new)
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
        _ => CoreError::Internal("unhandled login error"),
    }
}

fn map_email_error(err: TwoFactorEmailError) -> CoreError {
    match err {
        TwoFactorEmailError::Api(api) => map_api_error(&api),
        TwoFactorEmailError::Prelogin(PreloginError::Api(api)) => map_api_error(&api),
        TwoFactorEmailError::Prelogin(PreloginError::MissingField(_)) => CoreError::InvalidResponse,
        TwoFactorEmailError::Crypto(_) => CoreError::Internal("email code encryption"),
    }
}

fn map_get_error(err: GetCipherError) -> CoreError {
    match err {
        GetCipherError::ItemNotFound(_) => CoreError::NotFound,
        GetCipherError::Crypto(_) => CoreError::Internal("decrypt the item"),
        GetCipherError::Repository(_) => CoreError::Internal("read the item from the repository"),
    }
}

fn map_sync_error(err: SyncError) -> CoreError {
    match err {
        SyncError::Api(api) => map_api_error(&api),
        SyncError::AccountDeleted => CoreError::AccountDeleted,
        SyncError::InvalidRevisionDate => CoreError::InvalidResponse,
        _ => CoreError::Internal("unhandled sync error"),
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
    use crate::model::ItemKind;
    use bitwarden_vault::{Cipher, CipherView, EncryptMode};

    #[test]
    fn client_constructs() {
        let vault = Vault::connect(None).unwrap();
        assert!(!vault.is_unlocked());
    }

    /// A cipher that cannot be decrypted is counted, not fatal: one bad item
    /// must not empty the list.
    #[tokio::test]
    async fn an_item_that_cannot_be_decrypted_is_counted_not_fatal() {
        let vault = Vault::connect(None).unwrap();
        let cipher: Cipher = serde_json::from_value(serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "collectionIds": [],
            "type": 1,
            "favorite": false,
            "reprompt": 0,
            "organizationUseTotp": false,
            "edit": false,
            "viewPassword": true,
            "creationDate": "2026-01-01T00:00:00Z",
            "revisionDate": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let id = cipher.id.expect("the fixture carries an id");
        vault
            .client
            .0
            .platform()
            .state()
            .get::<Cipher>()
            .unwrap()
            .set(id, cipher)
            .await
            .unwrap();

        // The client has no user key, so nothing can be decrypted.
        let snapshot = vault.snapshot().await.unwrap();

        assert!(snapshot.items.is_empty());
        assert_eq!(snapshot.decrypt_failures, 1);
    }

    /// An encrypted login in the in-memory repository, the way a sync leaves it.
    async fn add_login(vault: &Vault) {
        let view: CipherView = serde_json::from_value(serde_json::json!({
            "id": "22222222-2222-2222-2222-222222222222",
            "collectionIds": [],
            "name": "GitHub",
            "type": 1,
            "login": {
                "username": "octocat",
                "uris": [{ "uri": "https://github.com" }]
            },
            "favorite": false,
            "reprompt": 0,
            "organizationUseTotp": false,
            "edit": false,
            "viewPassword": true,
            "creationDate": "2026-01-01T00:00:00Z",
            "revisionDate": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        let cipher = vault
            .client
            .0
            .internal
            .get_key_store()
            .encrypt(EncryptMode::Legacy(view))
            .unwrap();
        let id = cipher.id.expect("the fixture carries an id");
        vault
            .client
            .0
            .platform()
            .state()
            .get::<Cipher>()
            .unwrap()
            .set(id, cipher)
            .await
            .unwrap();
    }

    /// The list keeps what the row and the search need: name, user name and
    /// URIs. A field lost here would silently break search.
    #[tokio::test]
    async fn a_listed_item_keeps_its_name_username_and_uri() {
        let vault = Vault::restore(&saved_session(), PASSWORD).await.unwrap();
        add_login(&vault).await;

        let snapshot = vault.snapshot().await.unwrap();

        let item = snapshot.items.first().expect("the item is listed");
        assert_eq!(snapshot.decrypt_failures, 0);
        assert_eq!(item.name, "GitHub");
        assert_eq!(item.username, "octocat");
        assert_eq!(item.uris, ["https://github.com"]);
        assert_eq!(item.subtitle, "octocat");
        assert!(item.search.contains("octocat"));
        assert_eq!(item.kind, ItemKind::Login);
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

    const EMAIL: &str = "test@example.com";
    const PASSWORD: &str = "a master password";

    /// A saved session built the way a real sign-in would build it.
    fn saved_session() -> SavedSession {
        use bitwarden_crypto::{PublicKeyEncryptionAlgorithm, SymmetricKeyAlgorithm};

        let store: bitwarden_crypto::KeyStore<bitwarden_core::key_management::KeySlotIds> =
            bitwarden_crypto::KeyStore::default();
        let mut ctx = store.context_mut();
        let user_key_id = ctx.make_symmetric_key(SymmetricKeyAlgorithm::Aes256CbcHmac);
        let private_key_id = ctx.make_private_key(PublicKeyEncryptionAlgorithm::RsaOaepSha1);
        let crypto_state = WrappedAccountCryptographicState::V1 {
            private_key: ctx.wrap_private_key(user_key_id, private_key_id).unwrap(),
        };
        #[allow(deprecated)]
        let user_key = ctx
            .dangerous_get_symmetric_key(user_key_id)
            .unwrap()
            .clone();

        let kdf = bitwarden_crypto::Kdf::PBKDF2 {
            iterations: std::num::NonZeroU32::new(5000).unwrap(),
        };
        let master_key = MasterKey::derive(PASSWORD, EMAIL, &kdf).unwrap();

        SavedSession {
            email: EMAIL.to_string(),
            user_id: UserId::new_v4(),
            identity_url: "https://vault.example.com/identity".to_string(),
            api_url: "https://vault.example.com/api".to_string(),
            tokens: AuthenticationTokens {
                access_token: "access".to_string(),
                refresh_token: Some("refresh".to_string()),
                expires_on: 0,
            },
            crypto_state,
            unlock: MasterPasswordUnlockData {
                kdf,
                salt: EMAIL.to_string(),
                master_key_wrapped_user_key: master_key.encrypt_user_key(&user_key).unwrap(),
                contained_key_id: None,
            },
        }
    }

    #[tokio::test]
    async fn a_saved_session_unlocks_with_the_master_password() {
        let vault = Vault::restore(&saved_session(), PASSWORD).await.unwrap();
        assert!(vault.is_unlocked());
    }

    #[tokio::test]
    async fn a_restored_session_keeps_the_password_login_method() {
        let vault = Vault::restore(&saved_session(), PASSWORD).await.unwrap();
        let method = vault
            .client
            .0
            .platform()
            .state()
            .setting(USER_LOGIN_METHOD)
            .unwrap()
            .get()
            .await
            .unwrap()
            .unwrap();
        let UserLoginMethod::Username {
            client_id, email, ..
        } = method
        else {
            panic!("expected a username login method");
        };
        // The refresh request sends this id to the identity server.
        assert_eq!(client_id, "web");
        assert_eq!(email, EMAIL);
    }

    #[tokio::test]
    async fn a_locked_vault_keeps_its_items_for_the_next_unlock() {
        let saved = saved_session();
        let vault = Vault::restore(&saved, PASSWORD).await.unwrap();
        add_login(&vault).await;
        assert_eq!(vault.snapshot().await.unwrap().items.len(), 1);

        vault.lock().await;
        assert!(!vault.is_unlocked());
        // The encrypted item is still there, but without the keys it cannot be
        // read. The next unlock makes it visible again without a sync.
        let locked = vault.snapshot().await.unwrap();
        assert!(locked.items.is_empty());
        assert_eq!(locked.decrypt_failures, 1);

        vault.unlock(&saved, PASSWORD).await.unwrap();
        assert!(vault.is_unlocked());
        let snapshot = vault.snapshot().await.unwrap();
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].name, "GitHub");
        assert_eq!(snapshot.decrypt_failures, 0);
    }

    #[tokio::test]
    async fn a_wrong_password_leaves_a_retained_vault_locked() {
        let saved = saved_session();
        let vault = Vault::restore(&saved, PASSWORD).await.unwrap();
        add_login(&vault).await;
        vault.lock().await;

        assert_eq!(
            vault.unlock(&saved, "not the password").await.err(),
            Some(CoreError::InvalidCredentials)
        );
        assert!(!vault.is_unlocked());

        vault.unlock(&saved, PASSWORD).await.unwrap();
        assert_eq!(vault.snapshot().await.unwrap().items.len(), 1);
    }

    #[tokio::test]
    async fn a_wrong_password_leaves_a_saved_session_locked() {
        assert_eq!(
            Vault::restore(&saved_session(), "not the password")
                .await
                .err(),
            Some(CoreError::InvalidCredentials)
        );
    }

    #[tokio::test]
    async fn renewed_tokens_replace_the_saved_ones() {
        let mut saved = saved_session();
        let vault = Vault::restore(&saved, PASSWORD).await.unwrap();
        // A restore installs what the file held, so nothing has changed yet.
        assert!(!vault.update_saved_tokens(&mut saved).await.unwrap());

        vault
            .client
            .0
            .platform()
            .state()
            .setting(AUTHENTICATION_TOKENS)
            .unwrap()
            .update(AuthenticationTokens {
                access_token: "renewed".to_string(),
                refresh_token: Some("renewed refresh".to_string()),
                expires_on: 1234,
            })
            .await
            .unwrap();

        assert!(vault.update_saved_tokens(&mut saved).await.unwrap());
        assert_eq!(saved.tokens.access_token, "renewed");
        assert_eq!(
            saved.tokens.refresh_token.as_deref(),
            Some("renewed refresh")
        );
        assert_eq!(saved.tokens.expires_on, 1234);
    }

    #[tokio::test]
    async fn a_vault_without_tokens_cannot_update_a_saved_login() {
        let vault = Vault::connect(None).unwrap();
        let mut saved = saved_session();
        assert!(vault.update_saved_tokens(&mut saved).await.is_err());
    }

    #[test]
    fn a_saved_session_belongs_to_one_account_and_server() {
        let saved = saved_session();
        assert!(saved.matches("https://vault.example.com", " TEST@example.com "));
        assert!(!saved.matches("https://vault.example.com", "other@example.com"));
        assert!(!saved.matches("", EMAIL));
        assert!(!saved.matches("http://vault.example.com", EMAIL));
    }

    #[test]
    fn a_saved_login_belongs_to_one_vault() {
        let saved = saved_session();
        let vault = Vault::connect(Some("https://vault.example.com")).unwrap();
        assert!(vault.owns(&saved, EMAIL));
        assert!(vault.owns(&saved, " TEST@example.com "));
        assert!(!vault.owns(&saved, "other@example.com"));

        let other = Vault::connect(Some("https://other.example.com")).unwrap();
        assert!(!other.owns(&saved, EMAIL));
    }

    #[test]
    fn a_saved_session_survives_a_round_trip() {
        let saved = saved_session();
        let text = serde_json::to_string(&saved).unwrap();
        let read: SavedSession = serde_json::from_str(&text).unwrap();

        assert_eq!(read.email, saved.email);
        assert_eq!(read.tokens.access_token, saved.tokens.access_token);
        assert_eq!(read.unlock.salt, saved.unlock.salt);
    }
}
