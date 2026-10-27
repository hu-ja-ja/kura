//! The rows the detail pane shows for one item.
//!
//! Each secret row carries its own flag, so revealing one does not reveal the
//! others.

use kura_core::{TotpCode, VaultItemDetail, openable_url};
use rust_i18n::t;

pub struct DetailRow {
    pub label: String,
    pub value: String,
    pub secret: bool,
    /// A safe URL to open in the browser.
    pub open_url: Option<String>,
    /// Shown after the value, e.g. the TOTP seconds left.
    pub suffix: Option<String>,
    pub multiline: bool,
    pub selectable: bool,
    pub copyable: bool,
    pub large: bool,
    /// What the copy button puts on the clipboard when it differs from the
    /// display text, as with a grouped TOTP code.
    pub copy_value: Option<String>,
}

impl DetailRow {
    fn new(label: String, value: String) -> Self {
        Self {
            label,
            value,
            secret: false,
            open_url: None,
            suffix: None,
            multiline: false,
            selectable: false,
            copyable: true,
            large: false,
            copy_value: None,
        }
    }

    fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    fn selectable(mut self) -> Self {
        self.selectable = true;
        self
    }

    fn large(mut self) -> Self {
        self.large = true;
        self
    }

    fn suffix(mut self, suffix: String) -> Self {
        self.suffix = Some(suffix);
        self
    }

    fn not_copyable(mut self) -> Self {
        self.copyable = false;
        self
    }

    fn copy_raw(mut self, value: String) -> Self {
        self.copy_value = Some(value);
        self
    }
}

pub fn rows(detail: &VaultItemDetail) -> Vec<DetailRow> {
    let mut rows = Vec::new();

    if let Some(username) = &detail.username {
        rows.push(DetailRow::new(
            t!("detail.username").to_string(),
            username.clone(),
        ));
    }
    if let Some(password) = &detail.password {
        rows.push(DetailRow::new(t!("detail.password").to_string(), password.clone()).secret());
    }
    for uri in &detail.uris {
        let mut row = DetailRow::new(t!("detail.uri").to_string(), uri.clone());
        row.open_url = openable_url(uri);
        rows.push(row);
    }
    if let Some(totp) = &detail.totp {
        rows.push(
            DetailRow::new(t!("detail.totp").to_string(), grouped_code(&totp.code))
                .large()
                .copy_raw(totp.code.clone())
                .suffix(t!("detail.totp_remaining", seconds = totp.remaining).to_string()),
        );
    }

    if let Some(card) = &detail.card {
        if let Some(value) = &card.cardholder {
            rows.push(DetailRow::new(
                t!("detail.cardholder").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &card.brand {
            rows.push(DetailRow::new(
                t!("detail.brand").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &card.number {
            rows.push(DetailRow::new(
                t!("detail.card_number").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &card.expiry {
            rows.push(DetailRow::new(
                t!("detail.expiry").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &card.code {
            rows.push(
                DetailRow::new(t!("detail.security_code").to_string(), value.clone()).secret(),
            );
        }
    }

    if let Some(identity) = &detail.identity {
        if let Some(value) = &identity.name {
            rows.push(DetailRow::new(t!("detail.name").to_string(), value.clone()));
        }
        if let Some(value) = &identity.company {
            rows.push(DetailRow::new(
                t!("detail.company").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &identity.email {
            rows.push(DetailRow::new(
                t!("detail.email").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &identity.phone {
            rows.push(DetailRow::new(
                t!("detail.phone").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &identity.username {
            rows.push(DetailRow::new(
                t!("detail.username").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &identity.address {
            rows.push(DetailRow::new(
                t!("detail.address").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &identity.ssn {
            rows.push(DetailRow::new(t!("detail.ssn").to_string(), value.clone()).secret());
        }
    }

    if let Some(key) = &detail.ssh_key {
        if let Some(value) = &key.public_key {
            rows.push(DetailRow::new(
                t!("detail.public_key").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &key.fingerprint {
            rows.push(DetailRow::new(
                t!("detail.fingerprint").to_string(),
                value.clone(),
            ));
        }
        if let Some(value) = &key.private_key {
            rows.push(DetailRow::new(t!("detail.private_key").to_string(), value.clone()).secret());
        }
    }

    if let Some(notes) = &detail.notes {
        rows.push(
            DetailRow::new(t!("detail.notes").to_string(), notes.clone())
                .multiline()
                .selectable(),
        );
    }

    for field in &detail.fields {
        let label = if field.name.trim().is_empty() {
            t!("detail.custom_field").to_string()
        } else {
            field.name.clone()
        };
        let mut row = DetailRow::new(label, field.value.clone());
        if field.secret {
            row = row.secret();
        }
        rows.push(row);
    }

    for attachment in &detail.attachments {
        let value = if attachment.size.is_empty() {
            attachment.name.clone()
        } else {
            format!("{} ({})", attachment.name, attachment.size)
        };
        rows.push(DetailRow::new(t!("detail.attachment").to_string(), value).not_copyable());
    }

    rows
}

/// Keep the digits from pasted text, for the OTP input.
pub fn otp_digits(text: &str, length: usize) -> String {
    text.chars()
        .filter_map(digit)
        .filter_map(|digit| char::from_digit(digit, 10))
        .take(length)
        .collect()
}

fn digit(character: char) -> Option<u32> {
    character.to_digit(10).or_else(|| {
        let digit = (character as u32).checked_sub('０' as u32)?;
        (digit < 10).then_some(digit)
    })
}

/// Advance the countdown by one second. Returns true when a new code is due.
pub fn tick_totp(totp: &mut TotpCode) -> bool {
    if totp.remaining == 0 {
        return false;
    }
    totp.remaining -= 1;
    totp.remaining == 0
}

/// Split a six-digit code for reading aloud. Other lengths stay as they are.
pub fn grouped_code(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    if chars.len() == 6 {
        format!(
            "{} {}",
            chars[..3].iter().collect::<String>(),
            chars[3..].iter().collect::<String>()
        )
    } else {
        code.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kura_core::{CustomField, ItemKind, TotpCode};

    fn detail() -> VaultItemDetail {
        VaultItemDetail {
            id: "item-1".into(),
            name: "GitHub".into(),
            kind: ItemKind::Login,
            favorite: false,
            folder_id: None,
            username: Some("octocat".into()),
            password: Some("hunter2".into()),
            uris: vec![
                "https://github.com/login".into(),
                "javascript:alert(1)".into(),
            ],
            totp: Some(TotpCode {
                code: "123456".into(),
                remaining: 30,
            }),
            notes: Some("line one\nline two".into()),
            fields: vec![CustomField {
                name: "PIN".into(),
                value: "1234".into(),
                secret: true,
            }],
            card: None,
            identity: None,
            ssh_key: None,
            attachments: vec![],
        }
    }

    #[test]
    fn each_secret_keeps_its_own_flag() {
        let rows = rows(&detail());
        let mut secret_values: Vec<_> = rows
            .iter()
            .filter(|row| row.secret)
            .map(|row| row.value.as_str())
            .collect();
        secret_values.sort_unstable();
        assert_eq!(secret_values, vec!["1234", "hunter2"]);
    }

    #[test]
    fn only_web_uris_get_an_open_action() {
        let rows = rows(&detail());
        let web = rows
            .iter()
            .find(|row| row.value == "https://github.com/login")
            .unwrap();
        assert!(web.open_url.is_some());
        let bad = rows
            .iter()
            .find(|row| row.value == "javascript:alert(1)")
            .unwrap();
        assert!(bad.open_url.is_none());
    }

    #[test]
    fn card_and_ssh_secrets_get_their_own_flags() {
        let mut value = detail();
        value.password = None;
        value.fields.clear();
        value.card = Some(kura_core::CardDetail {
            code: Some("999".into()),
            ..Default::default()
        });
        value.ssh_key = Some(kura_core::SshKeyDetail {
            private_key: Some("KEY".into()),
            ..Default::default()
        });

        let rows = rows(&value);
        let secret_values: Vec<_> = rows
            .iter()
            .filter(|row| row.secret)
            .map(|row| row.value.as_str())
            .collect();
        assert_eq!(secret_values, vec!["999", "KEY"]);
    }

    #[test]
    fn groups_only_six_digit_codes() {
        assert_eq!(grouped_code("123456"), "123 456");
        assert_eq!(grouped_code("12345"), "12345");
    }

    #[test]
    fn totp_copies_the_raw_code() {
        let rows = rows(&detail());
        let row = rows.iter().find(|row| row.large).unwrap();
        assert_eq!(row.value, "123 456");
        assert_eq!(row.copy_value.as_deref(), Some("123456"));
    }

    #[test]
    fn notes_are_selectable_text() {
        let rows = rows(&detail());
        let notes = rows.iter().find(|row| row.selectable).unwrap();
        assert!(notes.multiline);
        assert_eq!(notes.value, "line one\nline two");
    }

    #[test]
    fn otp_paste_keeps_digits_only() {
        assert_eq!(otp_digits("123 456", 6), "123456");
        assert_eq!(otp_digits("your code is 123456.", 6), "123456");
        assert_eq!(otp_digits("１２３４５６", 6), "123456");
        assert_eq!(otp_digits("1234567", 6), "123456");
        assert_eq!(otp_digits("no digits", 6), "");
    }

    #[test]
    fn totp_ticks_request_only_at_zero() {
        let mut totp = TotpCode {
            code: "123456".into(),
            remaining: 2,
        };
        assert!(!tick_totp(&mut totp));
        assert_eq!(totp.remaining, 1);
        assert!(tick_totp(&mut totp));
        assert_eq!(totp.remaining, 0);
        assert!(!tick_totp(&mut totp));
    }
}
