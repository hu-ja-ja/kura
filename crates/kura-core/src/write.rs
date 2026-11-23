//! Login-item drafts and the conversion to SDK write requests.
//!
//! The UI never sees an SDK type. An edit starts from a fresh, complete
//! `CipherView`; this module changes only the fields the form owns so passkeys,
//! attachments, organization data, and password history survive.

#[cfg(test)]
mod protocol_tests;

use std::collections::HashSet;

use bitwarden_vault::{
    CipherCreateRequest, CipherEditRequest, CipherRepromptType, CipherType, CipherView,
    CipherViewType, FieldType, FieldView, LoginUriView, LoginView, UriMatchType,
};
use zeroize::Zeroizing;

use crate::error::CoreError;

/// The login fields the editor owns. The item type is fixed to login.
#[derive(Clone)]
pub struct LoginDraft {
    pub name: String,
    pub username: String,
    pub password: Zeroizing<String>,
    pub totp: Zeroizing<String>,
    pub uris: Vec<LoginUriDraft>,
    pub notes: Zeroizing<String>,
    pub folder_id: Option<String>,
    pub favorite: bool,
    pub fields: Vec<LoginFieldDraft>,
}

impl Default for LoginDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            username: String::new(),
            password: Zeroizing::new(String::new()),
            totp: Zeroizing::new(String::new()),
            uris: Vec::new(),
            notes: Zeroizing::new(String::new()),
            folder_id: None,
            favorite: false,
            fields: Vec::new(),
        }
    }
}

/// The URI match rule, mirrored so the app does not name an SDK type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UriMatch {
    Domain,
    Host,
    StartsWith,
    Exact,
    RegularExpression,
    Never,
}

impl UriMatch {
    fn from_sdk(value: UriMatchType) -> Self {
        match value {
            UriMatchType::Domain => Self::Domain,
            UriMatchType::Host => Self::Host,
            UriMatchType::StartsWith => Self::StartsWith,
            UriMatchType::Exact => Self::Exact,
            UriMatchType::RegularExpression => Self::RegularExpression,
            UriMatchType::Never => Self::Never,
        }
    }

    fn to_sdk(self) -> UriMatchType {
        match self {
            Self::Domain => UriMatchType::Domain,
            Self::Host => UriMatchType::Host,
            Self::StartsWith => UriMatchType::StartsWith,
            Self::Exact => UriMatchType::Exact,
            Self::RegularExpression => UriMatchType::RegularExpression,
            Self::Never => UriMatchType::Never,
        }
    }
}

#[derive(Clone)]
pub struct LoginUriDraft {
    pub uri: String,
    pub match_type: Option<UriMatch>,
}

/// The custom-field types the form can edit. Linked fields are read-only and
/// are preserved from the fresh view rather than rebuilt from this draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginFieldKind {
    Text,
    Hidden,
    Boolean,
}

impl LoginFieldKind {
    fn from_sdk(value: FieldType) -> Option<Self> {
        match value {
            FieldType::Text => Some(Self::Text),
            FieldType::Hidden => Some(Self::Hidden),
            FieldType::Boolean => Some(Self::Boolean),
            FieldType::Linked => None,
        }
    }

    fn to_sdk(self) -> FieldType {
        match self {
            Self::Text => FieldType::Text,
            Self::Hidden => FieldType::Hidden,
            Self::Boolean => FieldType::Boolean,
        }
    }
}

#[derive(Clone)]
pub struct LoginFieldDraft {
    pub name: String,
    pub value: Zeroizing<String>,
    pub kind: LoginFieldKind,
    /// Where this field sat in the fresh view. New rows have no source.
    source: Option<usize>,
}

impl LoginFieldDraft {
    pub fn new(kind: LoginFieldKind) -> Self {
        Self {
            name: String::new(),
            value: Zeroizing::new(String::new()),
            kind,
            source: None,
        }
    }
}

/// Everything the edit form needs, captured from one fresh view.
#[derive(Clone)]
pub struct LoginEditor {
    pub id: String,
    pub revision: String,
    pub draft: LoginDraft,
}

pub(crate) fn editor_from_view(mut view: CipherView) -> Result<LoginEditor, CoreError> {
    if view.r#type != CipherType::Login
        || view.partial
        || view.deleted_date.is_some()
        || !view.edit
        || view.reprompt != CipherRepromptType::None
    {
        return Err(CoreError::NotEditable);
    }
    let id = view
        .id
        .map(|id| id.to_string())
        .ok_or(CoreError::NotEditable)?;
    let revision = revision_text(&view);
    let login = view.login.take().ok_or(CoreError::NotEditable)?;
    let uris = login
        .uris
        .unwrap_or_default()
        .into_iter()
        .filter_map(|uri| {
            Some(LoginUriDraft {
                uri: uri.uri?,
                match_type: uri.r#match.map(UriMatch::from_sdk),
            })
        })
        .collect();
    let fields = view
        .fields
        .take()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(source, field)| {
            let kind = LoginFieldKind::from_sdk(field.r#type)?;
            let name = field.name.unwrap_or_default();
            let value = field.value.unwrap_or_default();
            if name.trim().is_empty() && value.is_empty() {
                return None;
            }
            Some(LoginFieldDraft {
                name,
                value: Zeroizing::new(value),
                kind,
                source: Some(source),
            })
        })
        .collect();

    Ok(LoginEditor {
        id,
        revision,
        draft: LoginDraft {
            name: view.name,
            username: login.username.unwrap_or_default(),
            password: Zeroizing::new(login.password.unwrap_or_default()),
            totp: Zeroizing::new(login.totp.unwrap_or_default()),
            uris,
            notes: Zeroizing::new(view.notes.unwrap_or_default()),
            folder_id: view.folder_id.map(|id| id.to_string()),
            favorite: view.favorite,
            fields,
        },
    })
}

pub(crate) fn revision_text(view: &CipherView) -> String {
    view.revision_date.to_rfc3339()
}

pub(crate) fn revision_matches(view: &CipherView, opened_revision: &str) -> bool {
    revision_text(view) == opened_revision
}

pub(crate) fn build_create_request(draft: &LoginDraft) -> Result<CipherCreateRequest, CoreError> {
    let name = draft_name(draft)?;
    Ok(CipherCreateRequest {
        organization_id: None,
        collection_ids: Vec::new(),
        folder_id: folder_id(draft)?,
        name,
        notes: present(&draft.notes),
        favorite: draft.favorite,
        reprompt: CipherRepromptType::None,
        r#type: CipherViewType::Login(login_view(draft)?),
        fields: fields(draft)?,
        archived_date: None,
    })
}

/// Apply the form to a fresh view, then convert it through the SDK's own
/// request builder. Fields the form does not own are left untouched.
/// Collection ids are absent from `CipherEditRequest`; the SDK merges the
/// write-return over the stored cipher, which keeps them.
pub(crate) fn build_edit_request(
    mut view: CipherView,
    draft: &LoginDraft,
) -> Result<CipherEditRequest, CoreError> {
    if view.r#type != CipherType::Login
        || view.partial
        || view.deleted_date.is_some()
        || !view.edit
        || view.reprompt != CipherRepromptType::None
    {
        return Err(CoreError::NotEditable);
    }
    let original_fields = view.fields.take().unwrap_or_default();
    let merged_fields = merge_fields(original_fields, &draft.fields)?;

    view.name = draft_name(draft)?;
    view.notes = present(&draft.notes);
    view.favorite = draft.favorite;
    view.folder_id = folder_id(draft)?;
    view.fields = Some(merged_fields);

    let login = view.login.as_mut().ok_or(CoreError::NotEditable)?;
    login.username = present(&draft.username);
    login.password = present_raw(&draft.password);
    login.totp = present_trimmed(&draft.totp);
    login.uris = Some(uri_views(draft));
    login.generate_checksums();

    CipherEditRequest::try_from(view).map_err(|_| CoreError::Internal("build the edit request"))
}

/// Whether the draft would leave every form-owned field exactly as it is.
pub(crate) fn draft_is_unchanged(view: &CipherView, draft: &LoginDraft) -> bool {
    if view.name.trim() != draft.name.trim()
        || view.favorite != draft.favorite
        || view.folder_id.map(|id| id.to_string()) != normalized_folder(draft)
        || present(view.notes.as_deref().unwrap_or_default()) != present(&draft.notes)
    {
        return false;
    }
    let Some(login) = view.login.as_ref() else {
        return false;
    };
    if present(login.username.as_deref().unwrap_or_default()) != present(&draft.username)
        || present_raw(login.password.as_deref().unwrap_or_default())
            != present_raw(&draft.password)
        || present_trimmed(login.totp.as_deref().unwrap_or_default())
            != present_trimmed(&draft.totp)
        || !uris_unchanged(login, draft)
    {
        return false;
    }
    fields_unchanged(view, draft)
}

fn draft_name(draft: &LoginDraft) -> Result<String, CoreError> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err(CoreError::InvalidInput("item name"));
    }
    Ok(name.to_string())
}

fn normalized_folder(draft: &LoginDraft) -> Option<String> {
    draft
        .folder_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn folder_id(draft: &LoginDraft) -> Result<Option<bitwarden_vault::FolderId>, CoreError> {
    normalized_folder(draft)
        .map(|id| {
            id.parse()
                .map_err(|_| CoreError::InvalidInput("item folder"))
        })
        .transpose()
}

fn present(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn present_raw(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn present_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn login_view(draft: &LoginDraft) -> Result<LoginView, CoreError> {
    let _ = draft_name(draft)?;
    Ok(LoginView {
        username: present(&draft.username),
        password: present_raw(&draft.password),
        password_revision_date: None,
        uris: Some(uri_views(draft)),
        totp: present_trimmed(&draft.totp),
        autofill_on_page_load: None,
        http_authentication_enabled: None,
        fido2_credentials: None,
    })
}

fn uri_views(draft: &LoginDraft) -> Vec<LoginUriView> {
    draft
        .uris
        .iter()
        .filter(|uri| !uri.uri.trim().is_empty())
        .map(|uri| LoginUriView {
            uri: Some(uri.uri.clone()),
            r#match: uri.match_type.map(UriMatch::to_sdk),
            uri_checksum: None,
        })
        .collect()
}

fn uris_unchanged(login: &LoginView, draft: &LoginDraft) -> bool {
    let original: Vec<_> = login
        .uris
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|uri| uri.uri.as_deref().is_some_and(|uri| !uri.trim().is_empty()))
        .collect();
    let draft: Vec<_> = draft
        .uris
        .iter()
        .filter(|uri| !uri.uri.trim().is_empty())
        .collect();
    original.len() == draft.len()
        && original.iter().zip(draft).all(|(original, draft)| {
            original.uri.as_deref() == Some(draft.uri.as_str())
                && original.r#match.map(UriMatch::from_sdk) == draft.match_type
        })
}

fn field_from_draft(draft: &LoginFieldDraft) -> Option<FieldView> {
    let name = present(&draft.name);
    let value = present_raw(&draft.value);
    if name.is_none() && value.is_none() {
        return None;
    }
    Some(FieldView {
        name,
        value,
        r#type: draft.kind.to_sdk(),
        linked_id: None,
    })
}

fn fields(draft: &LoginDraft) -> Result<Vec<FieldView>, CoreError> {
    Ok(draft.fields.iter().filter_map(field_from_draft).collect())
}

fn editable(field: &FieldView) -> bool {
    LoginFieldKind::from_sdk(field.r#type).is_some()
}

fn same_field(field: &FieldView, draft: &LoginFieldDraft) -> bool {
    present(field.name.as_deref().unwrap_or_default()) == present(&draft.name)
        && present_raw(field.value.as_deref().unwrap_or_default()) == present_raw(&draft.value)
        && field.r#type == draft.kind.to_sdk()
}

fn represented(field: &FieldView) -> bool {
    editable(field)
        && !(field
            .name
            .as_deref()
            .is_none_or(|name| name.trim().is_empty())
            && field.value.as_deref().is_none_or(str::is_empty))
}

fn merge_fields(
    mut original: Vec<FieldView>,
    drafts: &[LoginFieldDraft],
) -> Result<Vec<FieldView>, CoreError> {
    let represented_indices: HashSet<usize> = original
        .iter()
        .enumerate()
        .filter_map(|(ix, field)| represented(field).then_some(ix))
        .collect();
    let mut used = HashSet::new();
    let mut additions = Vec::new();

    for draft in drafts {
        let Some(field) = field_from_draft(draft) else {
            continue;
        };
        match draft.source {
            Some(source) => {
                if !represented_indices.contains(&source) || !used.insert(source) {
                    return Err(CoreError::InvalidInput("custom field source"));
                }
                original[source] = field;
            }
            None => additions.push(field),
        }
    }

    let mut fields = Vec::with_capacity(original.len() + additions.len());
    for (ix, field) in original.into_iter().enumerate() {
        if !represented_indices.contains(&ix) || used.contains(&ix) {
            fields.push(field);
        }
    }
    fields.extend(additions);
    Ok(fields)
}

fn fields_unchanged(view: &CipherView, draft: &LoginDraft) -> bool {
    let original = view.fields.as_deref().unwrap_or_default();
    let represented_indices: HashSet<usize> = original
        .iter()
        .enumerate()
        .filter_map(|(ix, field)| represented(field).then_some(ix))
        .collect();
    let mut used = HashSet::new();

    for draft in &draft.fields {
        if field_from_draft(draft).is_none() {
            continue;
        }
        let Some(source) = draft.source else {
            return false;
        };
        if !represented_indices.contains(&source)
            || !used.insert(source)
            || !original
                .get(source)
                .is_some_and(|existing| same_field(existing, draft))
        {
            return false;
        }
    }
    represented_indices.len() == used.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn rich_view() -> CipherView {
        serde_json::from_value(json!({
            "id": "22222222-2222-2222-2222-222222222222",
            "organizationId": "55555555-5555-5555-5555-555555555555",
            "folderId": "33333333-3333-3333-3333-333333333333",
            "collectionIds": [
                "66666666-6666-6666-6666-666666666666",
                "77777777-7777-7777-7777-777777777777"
            ],
            "key": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ==",
            "name": "GitHub",
            "notes": "personal account",
            "type": 1,
            "login": {
                "username": "octocat",
                "password": "hunter2",
                "passwordRevisionDate": "2026-01-02T03:04:05Z",
                "uris": [
                    { "uri": "https://github.com", "match": 0 },
                    { "uri": "https://gist.github.com", "match": 3 }
                ],
                "totp": "JBSWY3DPEHPK3PXP",
                "autofillOnPageLoad": true,
                "httpAuthenticationEnabled": true,
                "fido2Credentials": [{
                    "credentialId": "credential-id",
                    "keyType": "public-key",
                    "keyAlgorithm": "ES256",
                    "keyCurve": "P-256",
                    "keyValue": "private-key-material",
                    "rpId": "github.com",
                    "userHandle": null,
                    "userName": "octocat",
                    "counter": "0",
                    "rpName": null,
                    "userDisplayName": null,
                    "discoverable": "true",
                    "creationDate": "2026-01-03T00:00:00Z"
                }]
            },
            "favorite": true,
            "reprompt": 0,
            "organizationUseTotp": true,
            "edit": true,
            "viewPassword": true,
            "attachments": [{
                "id": "attachment-id",
                "url": "https://vault.example.com/attachment",
                "fileName": "notes.txt",
                "size": "12",
                "sizeName": "12 Bytes",
                "key": null
            }],
            "fields": [
                { "name": "PIN", "value": "1234", "type": 1 },
                { "name": "Plain", "value": "text", "type": 0 },
                { "name": "Flag", "value": "true", "type": 2 },
                { "name": "Linked user", "value": "octocat", "type": 3, "linkedId": 100 }
            ],
            "creationDate": "2026-01-01T00:00:00Z",
            "revisionDate": "2026-01-04T00:00:00.123Z",
            "archivedDate": "2026-01-05T00:00:00Z"
        }))
        .expect("the fixture is a CipherView")
    }

    fn draft(view: &CipherView) -> LoginDraft {
        editor_from_view(view.clone())
            .expect("the fixture is editable")
            .draft
    }

    fn request(view: CipherView, draft: &LoginDraft) -> CipherEditRequest {
        build_edit_request(view, draft).expect("the request is valid")
    }

    fn differences(left: &Value, right: &Value, path: &str, out: &mut Vec<String>) {
        match (left, right) {
            (Value::Object(left), Value::Object(right)) => {
                for (key, value) in left {
                    let next = format!("{path}/{key}");
                    match right.get(key) {
                        Some(other) => differences(value, other, &next, out),
                        None => out.push(next),
                    }
                }
                for key in right.keys().filter(|key| !left.contains_key(*key)) {
                    out.push(format!("{path}/{key}"));
                }
            }
            (Value::Array(left), Value::Array(right)) => {
                if left.len() != right.len() {
                    out.push(format!("{path}[]"));
                    return;
                }
                for (ix, (left, right)) in left.iter().zip(right).enumerate() {
                    differences(left, right, &format!("{path}/{ix}"), out);
                }
            }
            (left, right) if left != right => out.push(path.to_string()),
            _ => {}
        }
    }

    #[test]
    fn a_no_op_draft_keeps_every_field_the_form_does_not_own() {
        let view = rich_view();
        let draft = draft(&view);
        assert!(draft_is_unchanged(&view, &draft));

        let request = request(view.clone(), &draft);
        assert_eq!(request.name, view.name);
        assert_eq!(request.notes, view.notes);
        assert_eq!(request.favorite, view.favorite);
        assert_eq!(request.folder_id, view.folder_id);
        assert_eq!(request.organization_id, view.organization_id);
        assert_eq!(request.key, view.key);
        assert_eq!(request.reprompt, view.reprompt);
        assert_eq!(request.revision_date, view.revision_date);
        assert_eq!(request.archived_date, view.archived_date);
        assert_eq!(request.attachments.len(), 1);
        assert_eq!(
            request.attachments[0].file_name.as_deref(),
            Some("notes.txt")
        );
        let CipherViewType::Login(login) = request.r#type else {
            panic!("the request stays a login");
        };
        assert_eq!(login.username.as_deref(), Some("octocat"));
        assert_eq!(login.password.as_deref(), Some("hunter2"));
        assert_eq!(login.autofill_on_page_load, Some(true));
        assert_eq!(login.http_authentication_enabled, Some(true));
        assert_eq!(login.fido2_credentials.as_ref().map(Vec::len), Some(1));
        assert_eq!(request.fields.len(), 4);
        assert_eq!(request.fields[3].r#type, FieldType::Linked);
        assert!(request.fields[3].linked_id.is_some());
    }

    #[test]
    fn each_editable_field_changes_only_its_own_request_field() {
        type FieldChange = Box<dyn Fn(&mut LoginDraft)>;

        let base = serde_json::to_value(request(rich_view(), &draft(&rich_view()))).unwrap();
        let cases: Vec<(&str, FieldChange)> = vec![
            ("/name", Box::new(|draft| draft.name = "GitLab".into())),
            (
                "/type/login/username",
                Box::new(|draft| draft.username = "other".into()),
            ),
            (
                "/type/login/password",
                Box::new(|draft| draft.password = Zeroizing::new("new".into())),
            ),
            (
                "/type/login/totp",
                Box::new(|draft| draft.totp = Zeroizing::new("NEWSEED".into())),
            ),
            (
                "/type/login/uris/0/uri|/type/login/uris/0/uriChecksum",
                Box::new(|draft| draft.uris[0].uri = "https://example.com".into()),
            ),
            (
                "/type/login/uris/0/match",
                Box::new(|draft| draft.uris[0].match_type = Some(UriMatch::Exact)),
            ),
            (
                "/notes",
                Box::new(|draft| *draft.notes = "new notes".into()),
            ),
            (
                "/folderId",
                Box::new(|draft| {
                    draft.folder_id = Some("44444444-4444-4444-4444-444444444444".into())
                }),
            ),
            ("/favorite", Box::new(|draft| draft.favorite = false)),
            (
                "/fields/0/value",
                Box::new(|draft| draft.fields[0].value = Zeroizing::new("9999".into())),
            ),
            (
                "/fields/2/type",
                Box::new(|draft| draft.fields[2].kind = LoginFieldKind::Text),
            ),
        ];

        for (expected, change) in cases {
            let mut draft = draft(&rich_view());
            change(&mut draft);
            assert!(!draft_is_unchanged(&rich_view(), &draft));
            let changed =
                serde_json::to_value(request(rich_view(), &draft)).expect("request serializes");
            let mut paths = Vec::new();
            differences(&base, &changed, "", &mut paths);
            assert_eq!(
                paths.join("|"),
                expected,
                "change {expected} touched other fields"
            );
        }
    }

    #[test]
    fn empty_values_become_none_and_empty_rows_are_dropped() {
        let view = rich_view();
        let mut draft = draft(&view);
        draft.username.clear();
        draft.password.clear();
        draft.totp = Zeroizing::new("   ".into());
        draft.notes.clear();
        draft.uris.push(LoginUriDraft {
            uri: "   ".into(),
            match_type: None,
        });
        draft.fields[1].name.clear();
        draft.fields[1].value.clear();
        draft
            .fields
            .push(LoginFieldDraft::new(LoginFieldKind::Text));

        let request = request(view, &draft);
        let CipherViewType::Login(login) = request.r#type else {
            panic!("the request stays a login");
        };
        assert!(login.username.is_none());
        assert!(login.password.is_none());
        assert!(login.totp.is_none());
        assert_eq!(login.uris.as_ref().map(Vec::len), Some(2));
        assert!(request.notes.is_none());
        // The emptied source row is deleted; the empty new row is ignored.
        assert_eq!(request.fields.len(), 3);
    }

    #[test]
    fn every_uri_match_round_trips_through_the_draft() {
        for (sdk, own) in [
            (UriMatchType::Domain, UriMatch::Domain),
            (UriMatchType::Host, UriMatch::Host),
            (UriMatchType::StartsWith, UriMatch::StartsWith),
            (UriMatchType::Exact, UriMatch::Exact),
            (UriMatchType::RegularExpression, UriMatch::RegularExpression),
            (UriMatchType::Never, UriMatch::Never),
        ] {
            let mut view = rich_view();
            let login = view.login.as_mut().expect("the fixture is a login");
            login.uris.as_mut().expect("the fixture has URIs")[0].r#match = Some(sdk);
            let mut draft = draft(&view);
            assert_eq!(draft.uris[0].match_type, Some(own));

            draft.uris[0].match_type = Some(own);
            let request = request(view, &draft);
            let CipherViewType::Login(login) = request.r#type else {
                panic!("the request stays a login");
            };
            assert_eq!(login.uris.unwrap()[0].r#match, Some(sdk));
        }
    }

    #[test]
    fn a_duplicate_or_foreign_custom_field_source_is_refused() {
        let view = rich_view();
        let draft = draft(&view);

        let mut duplicate = draft.clone();
        duplicate.fields.push(duplicate.fields[0].clone());
        assert_eq!(
            build_edit_request(view.clone(), &duplicate).err(),
            Some(CoreError::InvalidInput("custom field source"))
        );

        let mut linked = draft.clone();
        linked.fields[0].source = Some(3);
        assert_eq!(
            build_edit_request(view.clone(), &linked).err(),
            Some(CoreError::InvalidInput("custom field source"))
        );

        let mut missing = draft;
        missing.fields[0].source = Some(99);
        assert_eq!(
            build_edit_request(view, &missing).err(),
            Some(CoreError::InvalidInput("custom field source"))
        );
    }

    #[test]
    fn surrounding_whitespace_and_empty_optional_values_are_still_a_no_op() {
        let mut view = rich_view();
        view.name = "  GitHub  ".to_string();
        view.notes = Some(String::new());
        let login = view.login.as_mut().expect("the fixture is a login");
        login.username = Some("  octocat  ".to_string());
        login.uris.as_mut().expect("URIs")[0].uri = Some("  https://github.com  ".to_string());
        view.fields.as_mut().expect("fields")[0].name = Some("  PIN  ".to_string());
        view.fields.as_mut().expect("fields")[2].name = Some("   ".to_string());

        let draft = draft(&view);
        assert!(draft_is_unchanged(&view, &draft));
    }

    #[test]
    fn a_custom_field_with_only_a_name_or_only_a_value_is_kept() {
        let mut view = rich_view();
        view.fields = Some(
            serde_json::from_value(json!([
                { "name": "Only name", "type": 0 },
                { "value": "only value", "type": 1 },
                { "type": 0 }
            ]))
            .expect("the field fixtures deserialize"),
        );
        let draft = draft(&view);
        assert_eq!(draft.fields.len(), 2);
        assert_eq!(draft.fields[0].kind, LoginFieldKind::Text);
        assert_eq!(draft.fields[1].kind, LoginFieldKind::Hidden);

        let request = request(view, &draft);
        assert_eq!(request.fields.len(), 3);
        assert_eq!(request.fields[0].name.as_deref(), Some("Only name"));
        assert_eq!(request.fields[0].value, None);
        assert_eq!(request.fields[1].name, None);
        assert_eq!(request.fields[1].value.as_deref(), Some("only value"));
        assert_eq!(request.fields[2].name, None);
        assert_eq!(request.fields[2].value, None);
    }

    #[test]
    fn empty_custom_fields_and_uri_text_are_preserved() {
        let mut view = rich_view();
        view.fields.as_mut().unwrap().push(
            serde_json::from_value(json!({ "type": 0 }))
                .expect("the empty custom field fixture deserializes"),
        );
        let login = view.login.as_mut().expect("the fixture is a login");
        login.uris.as_mut().expect("URIs")[0].uri = Some("  ^/login$  ".to_string());
        let original_fields = view.fields.as_ref().unwrap().len();

        let mut draft = draft(&view);
        assert!(draft_is_unchanged(&view, &draft));
        *draft.notes = "changed".to_string();
        let request = request(view, &draft);
        assert_eq!(request.fields.len(), original_fields);
        assert!(
            request
                .fields
                .iter()
                .any(|field| field.name.is_none() && field.value.is_none())
        );
        let CipherViewType::Login(login) = request.r#type else {
            panic!("the request stays a login");
        };
        assert_eq!(login.uris.unwrap()[0].uri.as_deref(), Some("  ^/login$  "));
    }

    #[test]
    fn linked_fields_are_read_only_and_keep_their_position() {
        let view = rich_view();
        let draft = draft(&view);
        assert_eq!(draft.fields.len(), 3);

        let request = request(view, &draft);
        let types: Vec<_> = request.fields.iter().map(|field| field.r#type).collect();
        assert_eq!(
            types,
            [
                FieldType::Hidden,
                FieldType::Text,
                FieldType::Boolean,
                FieldType::Linked
            ]
        );
    }

    #[test]
    fn non_logins_partial_deleted_and_read_only_items_fail_closed() {
        for mutate in [
            |view: &mut CipherView| view.r#type = CipherType::Card,
            |view: &mut CipherView| view.partial = true,
            |view: &mut CipherView| view.deleted_date = Some(view.revision_date),
            |view: &mut CipherView| view.edit = false,
            |view: &mut CipherView| view.login = None,
            |view: &mut CipherView| view.reprompt = CipherRepromptType::Password,
        ] {
            let mut view = rich_view();
            mutate(&mut view);
            assert_eq!(
                editor_from_view(view.clone()).err(),
                Some(CoreError::NotEditable)
            );
            let draft = draft(&rich_view());
            assert!(matches!(
                build_edit_request(view, &draft),
                Err(CoreError::NotEditable)
            ));
        }
    }

    #[test]
    fn a_name_is_required_and_trimmed_but_a_password_keeps_its_spaces() {
        let view = rich_view();
        let mut edited = draft(&view);
        edited.name = "  GitHub  ".into();
        edited.password = Zeroizing::new("  a password  ".into());
        let request = request(view, &edited);
        assert_eq!(request.name, "GitHub");
        let CipherViewType::Login(login) = request.r#type else {
            panic!("the request stays a login");
        };
        assert_eq!(login.password.as_deref(), Some("  a password  "));

        let mut changed_draft = draft(&rich_view());
        changed_draft.name = "   ".into();
        assert_eq!(
            build_edit_request(rich_view(), &changed_draft).err(),
            Some(CoreError::InvalidInput("item name"))
        );
        assert!(matches!(
            build_create_request(&changed_draft),
            Err(CoreError::InvalidInput("item name"))
        ));
    }

    #[test]
    fn a_create_request_has_no_identity_and_keeps_the_login_fields() {
        let mut draft = draft(&rich_view());
        draft.name = "New login".into();
        let request = build_create_request(&draft).expect("the draft is valid");
        assert!(request.organization_id.is_none());
        assert!(request.collection_ids.is_empty());
        assert_eq!(
            request.folder_id,
            draft.folder_id.map(|id| id.parse().unwrap())
        );
        assert_eq!(request.name, "New login");
        let CipherViewType::Login(login) = request.r#type else {
            panic!("the request is a login");
        };
        assert_eq!(login.username.as_deref(), Some("octocat"));
        assert_eq!(request.fields.len(), 3);
    }

    #[test]
    fn the_editor_revision_is_the_rendered_revision_date() {
        let view = rich_view();
        let editor = editor_from_view(view.clone()).unwrap();
        let expected = view.revision_date.to_rfc3339();
        assert_eq!(editor.id, view.id.unwrap().to_string());
        assert_eq!(editor.revision, expected);
        assert_eq!(revision_text(&view), expected);
        assert!(revision_matches(&view, &expected));
        assert!(!revision_matches(&view, "another revision"));
    }
}
