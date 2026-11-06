//! The shape of the vault the UI sees.
//!
//! The snapshot holds only what the list shows. Secrets are decrypted per item
//! on demand as a [`VaultItemDetail`] and dropped when the item is closed.

use std::time::{SystemTime, UNIX_EPOCH};

use bitwarden_vault::{
    CipherListView, CipherListViewType, CipherType, CipherView, FieldType, generate_totp,
};
use url::Url;
use zeroize::Zeroizing;

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
    /// Lowercase name, username and URIs joined by NUL, so a keystroke does
    /// not lower-case every field of every item again and a query can never
    /// match across two fields.
    pub search: String,
    pub kind: ItemKind,
    pub favorite: bool,
    pub folder_id: Option<String>,
}

impl From<CipherListView> for VaultItem {
    fn from(view: CipherListView) -> Self {
        // The list view carries no password or notes, except for blob-format
        // items, which the SDK unseals whole to build it.
        let (kind, username, uris) = match view.r#type {
            CipherListViewType::Login(login) => (
                ItemKind::Login,
                login.username.unwrap_or_default(),
                login
                    .uris
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|uri| uri.uri)
                    .collect(),
            ),
            CipherListViewType::Card(_) => (ItemKind::Card, String::new(), Vec::new()),
            CipherListViewType::Identity => (ItemKind::Identity, String::new(), Vec::new()),
            CipherListViewType::SecureNote => (ItemKind::SecureNote, String::new(), Vec::new()),
            CipherListViewType::SshKey => (ItemKind::SshKey, String::new(), Vec::new()),
            CipherListViewType::BankAccount(_)
            | CipherListViewType::DriversLicense
            | CipherListViewType::Passport => (ItemKind::Other, String::new(), Vec::new()),
        };

        let subtitle = if username.is_empty() {
            uris.first().cloned().unwrap_or_default()
        } else {
            username.clone()
        };

        let mut search = view.name.to_lowercase();
        for part in std::iter::once(&username).chain(uris.iter()) {
            search.push('\0');
            search.push_str(&part.to_lowercase());
        }

        Self {
            id: view.id.map(|id| id.to_string()).unwrap_or_default(),
            name: view.name,
            subtitle,
            username,
            uris,
            search,
            kind,
            favorite: view.favorite,
            folder_id: view.folder_id.map(|id| id.to_string()),
        }
    }
}

/// A folder shown in the filter menu.
#[derive(Clone)]
pub struct Folder {
    pub id: String,
    pub name: String,
}

/// The whole list, as of the last sync.
#[derive(Clone, Default)]
pub struct VaultSnapshot {
    pub items: Vec<VaultItem>,
    pub folders: Vec<Folder>,
    /// Items the SDK could not decrypt. They are not shown.
    pub decrypt_failures: usize,
}

/// One generated code and the seconds left before it changes.
#[derive(Clone)]
pub struct TotpCode {
    pub code: String,
    pub remaining: u32,
}

#[derive(Clone)]
pub struct CustomField {
    pub name: String,
    pub value: Zeroizing<String>,
    pub secret: bool,
}

#[derive(Clone, Default)]
pub struct CardDetail {
    pub cardholder: Option<String>,
    pub brand: Option<String>,
    pub number: Option<Zeroizing<String>>,
    pub expiry: Option<String>,
    pub code: Option<Zeroizing<String>>,
}

#[derive(Clone, Default)]
pub struct IdentityDetail {
    pub name: Option<String>,
    pub company: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub username: Option<String>,
    pub address: Option<String>,
    pub ssn: Option<Zeroizing<String>>,
}

#[derive(Clone, Default)]
pub struct SshKeyDetail {
    pub private_key: Option<Zeroizing<String>>,
    pub public_key: Option<String>,
    pub fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct AttachmentInfo {
    pub name: String,
    pub size: String,
}

/// Everything the detail pane shows for one item. Held only while it is open.
#[derive(Clone)]
pub struct VaultItemDetail {
    pub id: String,
    pub name: String,
    pub kind: ItemKind,
    pub favorite: bool,
    pub folder_id: Option<String>,
    pub username: Option<String>,
    pub password: Option<Zeroizing<String>>,
    pub uris: Vec<String>,
    pub totp: Option<TotpCode>,
    pub notes: Option<Zeroizing<String>>,
    pub fields: Vec<CustomField>,
    pub card: Option<CardDetail>,
    pub identity: Option<IdentityDetail>,
    pub ssh_key: Option<SshKeyDetail>,
    pub attachments: Vec<AttachmentInfo>,
}

impl From<CipherView> for VaultItemDetail {
    fn from(mut view: CipherView) -> Self {
        // Move the login fields out of the view. Cloning them would leave a
        // second, unzeroized copy of the password in the freed heap.
        let (username, password, uris, totp) = match view.login.take() {
            Some(login) => (
                non_empty(login.username),
                non_empty(login.password).map(Zeroizing::new),
                login
                    .uris
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|uri| non_empty(uri.uri))
                    .collect(),
                login
                    .totp
                    .map(Zeroizing::new)
                    .as_deref()
                    .and_then(|seed| totp_code(seed.as_str())),
            ),
            None => (None, None, Vec::new(), None),
        };

        let card = view.card.map(|card| CardDetail {
            cardholder: non_empty(card.cardholder_name),
            brand: non_empty(card.brand),
            number: non_empty(card.number).map(Zeroizing::new),
            expiry: match (non_empty(card.exp_month), non_empty(card.exp_year)) {
                (Some(month), Some(year)) => Some(format!("{month}/{year}")),
                (Some(month), None) => Some(month),
                (None, Some(year)) => Some(year),
                (None, None) => None,
            },
            code: non_empty(card.code).map(Zeroizing::new),
        });

        let identity = view.identity.map(|identity| IdentityDetail {
            name: join(
                [
                    identity.first_name,
                    identity.middle_name,
                    identity.last_name,
                ],
                " ",
            ),
            company: non_empty(identity.company),
            email: non_empty(identity.email),
            phone: non_empty(identity.phone),
            username: non_empty(identity.username),
            address: join(
                [
                    identity.address1,
                    identity.address2,
                    identity.address3,
                    identity.city,
                    identity.state,
                    identity.postal_code,
                    identity.country,
                ],
                ", ",
            ),
            ssn: non_empty(identity.ssn).map(Zeroizing::new),
        });

        let ssh_key = view.ssh_key.map(|key| SshKeyDetail {
            private_key: non_empty(Some(key.private_key)).map(Zeroizing::new),
            public_key: non_empty(Some(key.public_key)),
            fingerprint: non_empty(Some(key.fingerprint)),
        });

        let fields = view
            .fields
            .unwrap_or_default()
            .into_iter()
            .filter_map(|field| {
                let name = field.name.unwrap_or_default();
                let value = field.value.unwrap_or_default();
                if name.trim().is_empty() && value.trim().is_empty() {
                    return None;
                }
                Some(CustomField {
                    name,
                    value: Zeroizing::new(value),
                    secret: field.r#type == FieldType::Hidden,
                })
            })
            .collect();

        let attachments = view
            .attachments
            .unwrap_or_default()
            .into_iter()
            .filter_map(|attachment| {
                let name = non_empty(attachment.file_name)?;
                Some(AttachmentInfo {
                    name,
                    size: attachment.size_name.unwrap_or_default(),
                })
            })
            .collect();

        Self {
            id: view.id.map(|id| id.to_string()).unwrap_or_default(),
            name: view.name,
            kind: ItemKind::from(view.r#type),
            favorite: view.favorite,
            folder_id: view.folder_id.map(|id| id.to_string()),
            username,
            password,
            uris,
            totp,
            notes: non_empty(view.notes).map(Zeroizing::new),
            fields,
            card,
            identity,
            ssh_key,
            attachments,
        }
    }
}

/// Generate the current code from a stored TOTP key.
pub(crate) fn totp_for_view(view: &CipherView) -> Option<TotpCode> {
    view.login
        .as_ref()
        .and_then(|login| login.totp.as_deref())
        .and_then(totp_code)
}

fn totp_code(key: &str) -> Option<TotpCode> {
    let response = generate_totp(key.to_string(), None).ok()?;
    let period = response.period.max(1);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let remaining = period - (now % period as u64) as u32;
    Some(TotpCode {
        code: response.code,
        remaining,
    })
}

/// The URI to hand to the browser, after checking it is a plain web URL.
pub fn openable_url(uri: &str) -> Option<String> {
    let url = Url::parse(uri.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    // A URL with credentials would put them in the browser's address bar.
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.to_string())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn join(values: impl IntoIterator<Item = Option<String>>, separator: &str) -> Option<String> {
    let joined = values
        .into_iter()
        .flatten()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(separator);
    (!joined.is_empty()).then_some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_plain_web_urls() {
        assert_eq!(
            openable_url("https://example.com/login"),
            Some("https://example.com/login".to_string())
        );
        assert!(openable_url("https://user:pass@example.com").is_none());
        assert!(openable_url("javascript:alert(1)").is_none());
        assert!(openable_url("file:///C:/Windows").is_none());
        assert!(openable_url("not a url").is_none());
    }
}
