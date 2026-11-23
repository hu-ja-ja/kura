//! Password and passphrase generation.
//!
//! The Bitwarden SDK owns the algorithm, the character sets, the word list,
//! and the randomness. This module only maps the saved options onto its
//! requests so Kura generates exactly what Bitwarden clients generate.

use std::sync::OnceLock;

use bitwarden_core::Client;
use bitwarden_generators::{
    GeneratorClientsExt, PassphraseGeneratorRequest, PasswordGeneratorRequest,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const MIN_PASSWORD_LENGTH: u8 = bitwarden_generators::MINIMUM_PASSWORD_LENGTH;
pub const MAX_PASSWORD_LENGTH: u8 = bitwarden_generators::MAXIMUM_PASSWORD_LENGTH;
pub const MIN_PASSPHRASE_WORDS: u8 = bitwarden_generators::MINIMUM_PASSPHRASE_NUM_WORDS;
pub const MAX_PASSPHRASE_WORDS: u8 = bitwarden_generators::MAXIMUM_PASSPHRASE_NUM_WORDS;
pub const MAX_MIN_CHARS: u8 = bitwarden_generators::MAXIMUM_MIN_CHAR_COUNT;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorMode {
    #[default]
    Password,
    Passphrase,
}

/// The password options and defaults of the Bitwarden clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordOptions {
    pub length: u8,
    pub uppercase: bool,
    pub lowercase: bool,
    pub numbers: bool,
    pub special: bool,
    pub avoid_ambiguous: bool,
    /// The minimum number of digits when numbers are on. At least one.
    pub min_number: u8,
    /// The minimum number of symbols when symbols are on. At least one.
    pub min_special: u8,
}

impl Default for PasswordOptions {
    fn default() -> Self {
        Self {
            length: 14,
            uppercase: true,
            lowercase: true,
            numbers: true,
            special: false,
            avoid_ambiguous: false,
            min_number: 1,
            min_special: 1,
        }
    }
}

/// The passphrase options and defaults of the Bitwarden clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PassphraseOptions {
    pub words: u8,
    pub separator: String,
    pub capitalize: bool,
    pub include_number: bool,
}

impl Default for PassphraseOptions {
    fn default() -> Self {
        Self {
            words: 6,
            separator: "-".to_string(),
            capitalize: false,
            include_number: false,
        }
    }
}

/// The generator settings saved in `settings.json`. No generated value is
/// saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneratorOptions {
    pub mode: GeneratorMode,
    pub password: PasswordOptions,
    pub passphrase: PassphraseOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratorError {
    /// Every character set is off.
    NoCharacterSet,
    /// The SDK refused the request. The UI never builds one that it can refuse.
    InvalidOptions,
}

/// Generate with the Bitwarden SDK. The result is zeroized when dropped.
pub fn generate(options: &GeneratorOptions) -> Result<Zeroizing<String>, GeneratorError> {
    match options.mode {
        GeneratorMode::Password => generate_password(&options.password),
        GeneratorMode::Passphrase => generate_passphrase(&options.passphrase),
    }
}

/// The shortest length that still fits the selected classes and minimums.
pub fn minimum_password_length(options: &PasswordOptions) -> u8 {
    let min_number = if options.numbers {
        options.min_number.clamp(1, MAX_MIN_CHARS)
    } else {
        0
    };
    let min_special = if options.special {
        options.min_special.clamp(1, MAX_MIN_CHARS)
    } else {
        0
    };
    let required =
        u8::from(options.uppercase) + u8::from(options.lowercase) + min_number + min_special;
    required.max(MIN_PASSWORD_LENGTH)
}

/// The length the SDK will use: clamped to the client range and raised to fit
/// the selected classes and minimums.
pub fn effective_password_length(options: &PasswordOptions) -> u8 {
    options
        .length
        .clamp(MIN_PASSWORD_LENGTH, MAX_PASSWORD_LENGTH)
        .max(minimum_password_length(options))
}

fn generate_password(options: &PasswordOptions) -> Result<Zeroizing<String>, GeneratorError> {
    if !options.uppercase && !options.lowercase && !options.numbers && !options.special {
        return Err(GeneratorError::NoCharacterSet);
    }
    let request = PasswordGeneratorRequest {
        lowercase: options.lowercase,
        uppercase: options.uppercase,
        numbers: options.numbers,
        special: options.special,
        length: effective_password_length(options),
        avoid_ambiguous: options.avoid_ambiguous,
        // The SDK guarantees one character of every enabled class when the
        // minimum is absent, which is what the Bitwarden clients do.
        min_number: options
            .numbers
            .then(|| options.min_number.clamp(1, MAX_MIN_CHARS)),
        min_special: options
            .special
            .then(|| options.min_special.clamp(1, MAX_MIN_CHARS)),
        ..Default::default()
    };
    generator()
        .password(request)
        .map(Zeroizing::new)
        .map_err(|_| GeneratorError::InvalidOptions)
}

fn generate_passphrase(options: &PassphraseOptions) -> Result<Zeroizing<String>, GeneratorError> {
    let separator = if options.separator.is_empty() {
        "-"
    } else {
        options.separator.as_str()
    };
    let request = PassphraseGeneratorRequest {
        num_words: options
            .words
            .clamp(MIN_PASSPHRASE_WORDS, MAX_PASSPHRASE_WORDS),
        word_separator: separator.to_string(),
        capitalize: options.capitalize,
        include_number: options.include_number,
    };
    generator()
        .passphrase(request)
        .map(Zeroizing::new)
        .map_err(|_| GeneratorError::InvalidOptions)
}

fn generator() -> bitwarden_generators::GeneratorClient {
    client().generator()
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| Client::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(options: PasswordOptions) -> String {
        let options = GeneratorOptions {
            mode: GeneratorMode::Password,
            password: options,
            ..Default::default()
        };
        generate(&options)
            .expect("the default options are valid")
            .to_string()
    }

    fn passphrase(options: PassphraseOptions) -> String {
        let options = GeneratorOptions {
            mode: GeneratorMode::Passphrase,
            passphrase: options,
            ..Default::default()
        };
        generate(&options)
            .expect("the passphrase options are valid")
            .to_string()
    }

    #[test]
    fn default_password_matches_bitwarden_clients() {
        let value = password(PasswordOptions::default());

        assert_eq!(value.len(), 14);
        assert!(value.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(value.chars().any(|c| c.is_ascii_uppercase()));
        assert!(value.chars().any(|c| c.is_ascii_lowercase()));
        assert!(value.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn two_calls_give_different_passwords() {
        assert_ne!(
            password(PasswordOptions::default()),
            password(PasswordOptions::default())
        );
    }

    #[test]
    fn symbols_appear_when_enabled() {
        let value = password(PasswordOptions {
            special: true,
            min_special: 2,
            length: 24,
            ..Default::default()
        });

        let symbols = value.chars().filter(|c| "!@#$%^&*".contains(*c)).count();
        assert!(symbols >= 2, "{value} has only {symbols} symbols");
    }

    #[test]
    fn minimums_are_met() {
        let value = password(PasswordOptions {
            special: true,
            min_number: 5,
            min_special: 3,
            length: 32,
            ..Default::default()
        });

        assert!(value.chars().filter(char::is_ascii_digit).count() >= 5);
        assert!(value.chars().filter(|c| "!@#$%^&*".contains(*c)).count() >= 3);
    }

    #[test]
    fn avoid_ambiguous_excludes_the_bitwarden_set() {
        for _ in 0..32 {
            let value = password(PasswordOptions {
                length: 128,
                avoid_ambiguous: true,
                ..Default::default()
            });
            assert!(
                !value.chars().any(|c| "IOl01".contains(c)),
                "{value} contains an ambiguous character"
            );
        }
    }

    #[test]
    fn a_short_length_is_raised_to_fit_the_minimums() {
        let value = password(PasswordOptions {
            uppercase: false,
            lowercase: false,
            special: false,
            length: 5,
            min_number: 9,
            ..Default::default()
        });

        assert_eq!(value.len(), 9);
        assert!(value.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn the_length_is_clamped_to_the_client_bounds() {
        assert_eq!(
            password(PasswordOptions {
                length: 1,
                ..Default::default()
            })
            .len(),
            usize::from(MIN_PASSWORD_LENGTH)
        );
        assert_eq!(
            password(PasswordOptions {
                length: 255,
                ..Default::default()
            })
            .len(),
            usize::from(MAX_PASSWORD_LENGTH)
        );
    }

    #[test]
    fn no_character_set_is_rejected() {
        let options = GeneratorOptions {
            password: PasswordOptions {
                uppercase: false,
                lowercase: false,
                numbers: false,
                special: false,
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(generate(&options), Err(GeneratorError::NoCharacterSet));
    }

    #[test]
    fn default_passphrase_matches_bitwarden_clients() {
        let value = passphrase(PassphraseOptions::default());
        let words: Vec<_> = value.split('-').collect();

        assert_eq!(words.len(), 6);
        assert!(
            words
                .iter()
                .all(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()))
        );
        assert!(!value.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn passphrase_capitalizes_and_adds_one_number() {
        let value = passphrase(PassphraseOptions {
            capitalize: true,
            include_number: true,
            ..Default::default()
        });
        let words: Vec<_> = value.split('-').collect();

        assert_eq!(words.len(), 6);
        assert_eq!(value.chars().filter(char::is_ascii_digit).count(), 1);
        for word in words {
            let word = word.trim_end_matches(|c: char| c.is_ascii_digit());
            assert!(
                word.chars().next().is_some_and(char::is_uppercase),
                "{word} is not capitalized"
            );
        }
    }

    #[test]
    fn the_separator_joins_the_words() {
        let value = passphrase(PassphraseOptions {
            separator: " ".to_string(),
            ..Default::default()
        });

        assert_eq!(value.split(' ').count(), 6);
        assert!(!value.contains('-'));
    }

    #[test]
    fn an_empty_separator_falls_back_to_a_hyphen() {
        let value = passphrase(PassphraseOptions {
            separator: String::new(),
            ..Default::default()
        });

        assert_eq!(value.split('-').count(), 6);
    }

    #[test]
    fn the_word_count_is_clamped_to_the_client_bounds() {
        assert_eq!(
            passphrase(PassphraseOptions {
                words: 1,
                ..Default::default()
            })
            .split('-')
            .count(),
            usize::from(MIN_PASSPHRASE_WORDS)
        );
        assert_eq!(
            passphrase(PassphraseOptions {
                words: 255,
                ..Default::default()
            })
            .split('-')
            .count(),
            usize::from(MAX_PASSPHRASE_WORDS)
        );
    }

    #[test]
    fn settings_fill_missing_generator_fields_with_the_defaults() {
        let options: GeneratorOptions =
            serde_json::from_str(r#"{"mode":"passphrase"}"#).expect("valid settings");

        assert_eq!(options.mode, GeneratorMode::Passphrase);
        assert_eq!(options.password, PasswordOptions::default());
        assert_eq!(options.passphrase, PassphraseOptions::default());
    }
}
