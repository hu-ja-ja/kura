//! Release-only checks against a disposable Vaultwarden.
//!
//! These tests create and delete real server data. They run only with
//! `mise run test:live`, and refuse any server that is not on this machine.

use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitwarden_core::ClientSettings;
use bitwarden_vault::EditCipherError;
use url::Url;

use super::{LoginOutcome, Vault};
use crate::write::{
    LoginDraft, LoginFieldDraft, LoginFieldKind, LoginUriDraft, UriMatch, build_edit_request,
};
use zeroize::Zeroizing;

const URL: &str = "KURA_LIVE_URL";
const EMAIL: &str = "KURA_LIVE_EMAIL";
const PASSWORD: &str = "KURA_LIVE_PASSWORD";

fn settings() -> ClientSettings {
    let base = std::env::var(URL).unwrap_or_else(|_| panic!("set {URL}"));
    let url = Url::parse(&base).unwrap_or_else(|_| panic!("{URL} is not a URL"));
    assert_eq!(url.scheme(), "http", "{URL} must be the local test server");
    assert!(
        url.host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "[::1]")),
        "{URL} must point at this machine; refusing to write to a remote vault"
    );
    assert!(url.query().is_none() && url.fragment().is_none());
    let base = base.trim_end_matches('/');
    let mut settings = crate::client_settings(None).expect("the default client settings build");
    settings.identity_url = format!("{base}/identity");
    settings.api_url = format!("{base}/api");
    settings
}

fn credentials() -> (String, String) {
    (
        std::env::var(EMAIL).unwrap_or_else(|_| panic!("set {EMAIL}")),
        std::env::var(PASSWORD).unwrap_or_else(|_| panic!("set {PASSWORD}")),
    )
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

async fn restore_local(saved: &super::SavedSession, password: &str) -> Vault {
    use bitwarden_core::client::persisted_state::AUTHENTICATION_TOKENS;

    let vault = Vault::from_settings(settings()).expect("the live settings build");
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

async fn login() -> (Vault, String, String) {
    let (email, password) = credentials();
    let vault = Vault::from_settings(settings()).expect("the live settings build");
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
    let (vault, email, password) = login().await;
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
    let second = restore_local(&saved, &password).await;
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
