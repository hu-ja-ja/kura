//! Release-only checks against a disposable Vaultwarden.
//!
//! These tests create and delete real server data. They run only with
//! `mise run test:live`, and refuse any server that is not on this machine.

use std::net::TcpListener;
use std::num::NonZeroU32;
use std::process::Command as ProcessCommand;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitwarden_core::ClientSettings;
use bitwarden_crypto::Kdf;
use bitwarden_vault::EditCipherError;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use url::Url;

use super::{LoginOutcome, Vault};
use crate::write::{
    LoginDraft, LoginFieldDraft, LoginFieldKind, LoginUriDraft, UriMatch, build_edit_request,
};
use zeroize::Zeroizing;

const URL: &str = "KURA_LIVE_URL";
const IMAGE: &str = "vaultwarden/server:1.37.5";
const CONTAINER: &str = "kura-live";
const KDF_ITERATIONS: u32 = 600_000;

/// Validate the test server the same way the product refuses remote vaults.
fn check_base(base: &str) -> String {
    let url = Url::parse(base).unwrap_or_else(|_| panic!("the live test URL is not a URL"));
    assert_eq!(url.scheme(), "http", "the live test server must use http");
    assert!(
        url.host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "[::1]")),
        "the live test server must be on this machine"
    );
    assert!(url.path() == "/", "the live test URL must not have a path");
    assert!(
        url.query().is_none() && url.fragment().is_none(),
        "the live test URL must not have a query or fragment"
    );
    base.trim_end_matches('/').to_string()
}

#[test]
fn the_live_server_stays_on_this_machine() {
    assert_eq!(
        check_base("http://127.0.0.1:18080/"),
        "http://127.0.0.1:18080"
    );
}

#[test]
#[should_panic(expected = "on this machine")]
fn a_remote_live_server_is_refused() {
    check_base("http://192.0.2.1:18080");
}

fn settings(base: &str) -> ClientSettings {
    let base = check_base(base);
    let mut settings = crate::client_settings(None).expect("the default client settings build");
    settings.identity_url = format!("{base}/identity");
    settings.api_url = format!("{base}/api");
    settings
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build the live HTTP client")
}

/// Start a disposable Vaultwarden unless the caller supplied one.
struct Vaultwarden {
    base: String,
    container: bool,
}

impl Vaultwarden {
    async fn start() -> Self {
        if let Ok(base) = std::env::var(URL) {
            return Self {
                base: check_base(&base),
                container: false,
            };
        }

        ensure_podman();
        let port = free_port();
        let base = format!("http://127.0.0.1:{port}");
        let _ = ProcessCommand::new("podman")
            .args(["rm", "-f", CONTAINER])
            .output();
        let published = format!("127.0.0.1:{port}:80");
        let output = ProcessCommand::new("podman")
            .args([
                "run",
                "-d",
                "--rm",
                "--name",
                CONTAINER,
                "-p",
                &published,
                "-e",
                "SIGNUPS_ALLOWED=true",
                "-e",
                "I_REALLY_WANT_VOLATILE_STORAGE=true",
                "-e",
                "WEB_VAULT_ENABLED=false",
                IMAGE,
            ])
            .output()
            .expect("run podman");
        assert!(
            output.status.success(),
            "podman run failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // The guard must exist before the wait so a panic still removes it.
        Self {
            base: check_base(&base),
            container: true,
        }
    }

    async fn wait_until_ready(&self) {
        let client = http_client();
        for _ in 0..60 {
            if client
                .get(format!("{}/alive", self.base))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let logs = ProcessCommand::new("podman")
            .args(["logs", CONTAINER])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        panic!("Vaultwarden did not become ready: {logs}");
    }
}

impl Drop for Vaultwarden {
    fn drop(&mut self) {
        if self.container {
            let _ = ProcessCommand::new("podman")
                .args(["rm", "-f", CONTAINER])
                .output();
        }
    }
}

fn ensure_podman() {
    let list = ProcessCommand::new("podman")
        .args(["machine", "list", "--format", "{{.Name}}"])
        .output()
        .expect("podman is installed; run `mise run test:live`");
    assert!(
        list.status.success(),
        "podman machine list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    if String::from_utf8_lossy(&list.stdout).trim().is_empty() {
        eprintln!("creating the Podman machine (one-time VM download)");
        let status = ProcessCommand::new("podman")
            .args(["machine", "init", "--now"])
            .status()
            .expect("run podman machine init");
        assert!(status.success(), "podman machine init failed");
        return;
    }
    let info = ProcessCommand::new("podman").arg("info").output();
    if !info.is_ok_and(|output| output.status.success()) {
        let status = ProcessCommand::new("podman")
            .args(["machine", "start"])
            .status()
            .expect("run podman machine start");
        assert!(status.success(), "podman machine start failed");
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a free loopback port");
    let port = listener.local_addr().expect("read the free port").port();
    drop(listener);
    port
}

async fn register(vault: &Vault, base: &str, email: &str, password: &str) {
    let client = http_client();
    let response = client
        .post(format!(
            "{base}/identity/accounts/register/send-verification-email"
        ))
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "email": email, "name": "Kura Live Test" }).to_string())
        .send()
        .await
        .expect("send the registration email");
    let status = response.status();
    let text = response.text().await.expect("read the registration email");
    assert!(
        status.is_success(),
        "registration email failed: {status} {text}"
    );
    let token: String =
        serde_json::from_str(text.trim()).expect("the registration token is a JSON string");

    let kdf = Kdf::PBKDF2 {
        iterations: NonZeroU32::new(KDF_ITERATIONS).expect("the KDF iterations are non-zero"),
    };
    let keys = vault
        .client
        .0
        .auth()
        .make_register_keys(email.to_string(), password.to_string(), kdf)
        .expect("make the registration keys");
    let body = serde_json::json!({
        "email": email,
        "masterPasswordHash": keys.master_password_hash.to_string(),
        "key": keys.encrypted_user_key.to_string(),
        "kdf": 0,
        "kdfIterations": KDF_ITERATIONS,
        "keys": {
            "publicKey": keys.keys.public.to_string(),
            "encryptedPrivateKey": keys.keys.private.to_string(),
        },
        "emailVerificationToken": token,
    });
    let response = client
        .post(format!("{base}/identity/accounts/register/finish"))
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("finish the registration");
    let status = response.status();
    let text = response.text().await.expect("read the registration result");
    assert!(status.is_success(), "registration failed: {status} {text}");
}

fn unique_name() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is later than the epoch")
        .as_nanos();
    format!("kura-live-{}-{nonce}", std::process::id())
}

fn draft(name: String) -> LoginDraft {
    LoginDraft {
        name,
        username: "octocat".to_string(),
        password: Zeroizing::new("first password".to_string()),
        totp: Zeroizing::new("JBSWY3DPEHPK3PXP".to_string()),
        uris: vec![
            LoginUriDraft {
                uri: "https://github.com".to_string(),
                match_type: Some(UriMatch::Host),
            },
            LoginUriDraft {
                uri: "https://gist.github.com".to_string(),
                match_type: Some(UriMatch::Exact),
            },
        ],
        notes: Zeroizing::new("live note".to_string()),
        folder_id: None,
        favorite: true,
        fields: vec![
            field(LoginFieldKind::Hidden, "PIN", "1234"),
            field(LoginFieldKind::Text, "Plain", "text"),
            field(LoginFieldKind::Boolean, "Flag", "true"),
        ],
    }
}

fn field(kind: LoginFieldKind, name: &str, value: &str) -> LoginFieldDraft {
    let mut field = LoginFieldDraft::new(kind);
    field.name = name.to_string();
    field.value = Zeroizing::new(value.to_string());
    field
}

async fn restore_local(base: &str, saved: &super::SavedSession, password: &str) -> Vault {
    use bitwarden_core::client::persisted_state::AUTHENTICATION_TOKENS;

    let vault = Vault::from_settings(settings(base)).expect("the live settings build");
    vault
        .client
        .0
        .platform()
        .state()
        .value::<AUTHENTICATION_TOKENS>()
        .set(saved.tokens.clone())
        .await
        .expect("install the tokens");
    vault
        .install_user_id(saved.user_id)
        .await
        .expect("install the user id");
    vault.user_id_confirmed.store(true, Ordering::SeqCst);
    vault.enable_strict_decryption().await;
    vault
        .unlock(saved, password)
        .await
        .expect("unlock the restored client");
    vault
}

async fn register_and_login(server: &Vaultwarden) -> (Vault, String, String) {
    let email = format!("{}@example.com", unique_name());
    let password = "a disposable master password".to_string();
    // The SDK client installs the rustls provider that the plain HTTP client
    // also needs, so build it before the first readiness request.
    let vault = Vault::from_settings(settings(&server.base)).expect("the live settings build");
    server.wait_until_ready().await;
    register(&vault, &server.base, &email, &password).await;
    assert!(matches!(
        vault.login(&email, &password).await.expect("live sign-in"),
        LoginOutcome::LoggedIn
    ));
    vault.sync(false).await.expect("the initial sync");
    (vault, email, password)
}

#[tokio::test]
#[ignore = "release check against local Vaultwarden"]
async fn live_create_edit_conflict_and_delete_a_login() {
    let server = Vaultwarden::start().await;
    let (vault, email, password) = register_and_login(&server).await;
    let name = unique_name();
    let id = vault
        .create_login(&draft(name.clone()))
        .await
        .expect("create a login");

    let editor = vault.editor(&id).await.expect("read the created login");
    assert_eq!(editor.draft.name, name);
    assert_eq!(editor.draft.username, "octocat");
    assert_eq!(editor.draft.password.as_str(), "first password");
    assert_eq!(editor.draft.totp.as_str(), "JBSWY3DPEHPK3PXP");
    assert_eq!(
        editor.draft.uris[0].match_type,
        Some(UriMatch::Host),
        "URI match survives the server round trip"
    );
    assert_eq!(editor.draft.fields.len(), 3);

    // A no-op edit must not advance the revision or send a write.
    let no_op_revision = editor.revision.clone();
    vault
        .edit_login(&id, &no_op_revision, &editor.draft)
        .await
        .expect("a no-op edit succeeds");
    let after_no_op = vault.editor(&id).await.expect("read after no-op");
    assert_eq!(after_no_op.revision, no_op_revision);

    let mut changed = after_no_op.draft.clone();
    changed.name = format!("{name}-edited");
    changed.username = "renamed".to_string();
    changed.password = Zeroizing::new("second password".to_string());
    changed.totp = Zeroizing::new("GEZDGNBVGY3TQOJQ".to_string());
    changed.notes = Zeroizing::new("edited note".to_string());
    changed.favorite = false;
    changed.uris[0].uri = "https://example.com".to_string();
    changed.uris[0].match_type = Some(UriMatch::Domain);
    changed.fields[0].value = Zeroizing::new("9999".to_string());
    changed
        .fields
        .push(field(LoginFieldKind::Text, "Added", "new"));
    vault
        .edit_login(&id, &after_no_op.revision, &changed)
        .await
        .expect("edit every supported field");
    let read_back = vault.editor(&id).await.expect("read the edit");
    assert_eq!(read_back.draft.name, changed.name);
    assert_eq!(read_back.draft.username, changed.username);
    assert_eq!(read_back.draft.password.as_str(), "second password");
    assert_eq!(read_back.draft.uris[0].uri, "https://example.com");
    assert_eq!(read_back.draft.fields.len(), 4);
    assert_ne!(read_back.revision, after_no_op.revision);

    // A second client changes the item first. The stale editor must not
    // overwrite it.
    let saved = vault
        .saved_session(&email, &password)
        .await
        .expect("build the second client's session");
    let second = restore_local(&server.base, &saved, &password).await;
    second.sync(false).await.expect("the second client syncs");
    let stale = second.editor(&id).await.expect("open the second editor");
    // Vaultwarden allows a sub-second revision skew. Wait past it so the
    // deliberately unsynced request below is unambiguously stale.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut first_change = read_back.draft.clone();
    first_change.name = format!("{name}-first-client");
    vault
        .edit_login(&id, &read_back.revision, &first_change)
        .await
        .expect("the first client writes");
    let mut stale_change = stale.draft;
    stale_change.name = format!("{name}-stale-client");

    // Bypass the pre-save sync once to observe the server's own stale
    // revision response and verify that it is recognized as a conflict.
    let stale_view = second.cipher_view(&id).await.expect("stale local view");
    let stale_request =
        build_edit_request(stale_view, &stale_change).expect("build a stale edit request");
    let server_error = second
        .client
        .vault()
        .ciphers()
        .edit(stale_request)
        .await
        .expect_err("the server rejects a stale revision");
    let EditCipherError::Api(server_error) = server_error else {
        panic!("expected an API rejection, got {server_error:?}");
    };
    assert!(
        super::is_stale_revision(&server_error),
        "unrecognized stale revision response: {server_error:?}"
    );

    assert_eq!(
        second
            .edit_login(&id, &stale.revision, &stale_change)
            .await
            .err(),
        Some(crate::CoreError::Conflict)
    );
    assert_eq!(
        vault.editor(&id).await.expect("read the winner").draft.name,
        format!("{name}-first-client")
    );

    vault.delete_login(&id).await.expect("move to trash");
    assert!(
        vault
            .snapshot()
            .await
            .expect("snapshot after delete")
            .items
            .iter()
            .all(|item| item.id != id)
    );
    vault
        .delete_login(&id)
        .await
        .expect("a repeated delete is idempotent");
    second
        .sync(false)
        .await
        .expect("the second client sees the trash");
    second
        .delete_login(&id)
        .await
        .expect("a missing server item reconciles to success");
}
