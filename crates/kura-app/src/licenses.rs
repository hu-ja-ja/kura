//! The bundled third-party license list.
//!
//! `mise run licenses` writes `assets/licenses.txt`, one `@@` section per
//! license text in the order cargo-about emits them. Each section names the
//! license, the crates that use it and the text, so the settings screen can
//! answer "which license does this crate use?".
//!
//! A crate whose license expression needs several texts (`MIT AND Unicode-3.0`)
//! appears in more than one section; its list is the union, without duplicates.

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The widest line the settings dialog shows without clipping it.
const WRAP: usize = 80;

const BUNDLED: &str = include_str!("../assets/licenses.txt");

pub struct License {
    pub name: String,
    pub text: String,
}

/// One row of the license view.
pub struct Line {
    pub text: String,
    /// The license name above its text, drawn stronger than the body.
    pub heading: bool,
}

pub struct Crate {
    pub name: String,
    pub version: String,
    pub licenses: Vec<License>,
}

impl Crate {
    /// The names of its licenses, in the order cargo-about emitted them.
    pub fn license_names(&self) -> String {
        let mut names: Vec<&str> = Vec::new();
        for license in &self.licenses {
            if !names.contains(&license.name.as_str()) {
                names.push(&license.name);
            }
        }
        names.join(", ")
    }

    /// Every license as fixed-height rows, each name above its text.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        for license in &self.licenses {
            if !lines.is_empty() {
                lines.push(Line {
                    text: String::new(),
                    heading: false,
                });
            }
            lines.push(Line {
                text: license.name.clone(),
                heading: true,
            });
            lines.push(Line {
                text: String::new(),
                heading: false,
            });
            lines.extend(wrap(&license.text).into_iter().map(|text| Line {
                text,
                heading: false,
            }));
        }
        lines
    }
}

/// The crates, sorted by name and version. Parsed on first use.
pub fn crates() -> &'static [Crate] {
    static CRATES: OnceLock<Vec<Crate>> = OnceLock::new();
    CRATES.get_or_init(|| parse(BUNDLED))
}

/// Split into the fixed-height rows the settings dialog scrolls through.
fn wrap(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim_end();
        while rest.chars().count() > WRAP {
            let cut = rest
                .char_indices()
                .nth(WRAP)
                .map_or(rest.len(), |(byte, _)| byte);
            // Break on a space when one is close to the edge, so words stay whole.
            let space = rest[..cut].rfind(' ').filter(|space| *space > cut / 2);
            let (head, tail) = match space {
                Some(space) => (&rest[..space], &rest[space + 1..]),
                None => (&rest[..cut], &rest[cut..]),
            };
            lines.push(head.to_string());
            rest = tail;
        }
        lines.push(rest.to_string());
    }
    lines
}

fn parse(text: &str) -> Vec<Crate> {
    // A Windows checkout can hand the file over with CRLF endings.
    let text = text.replace("\r\n", "\n");
    let mut crates: BTreeMap<(String, String), Vec<License>> = BTreeMap::new();

    // The leading newline makes the first marker split like every other one.
    for chunk in format!("\n{text}").split("\n@@ ") {
        let Some((name, body)) = chunk.split_once('\n') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let (crates_text, license_text) = body.split_once("\n\n").unwrap_or(("", body));
        let license_text = license_text.trim();
        for used_by in crates_text
            .lines()
            .filter_map(|line| line.strip_prefix(" - "))
        {
            let (crate_name, version) = used_by.rsplit_once(' ').unwrap_or((used_by, ""));
            let licenses = crates
                .entry((crate_name.to_string(), version.to_string()))
                .or_default();
            let is_new = !licenses
                .iter()
                .any(|license| license.name == name && license.text == license_text);
            if is_new {
                licenses.push(License {
                    name: name.to_string(),
                    text: license_text.to_string(),
                });
            }
        }
    }

    if crates.is_empty() {
        // A file without sections still shows its text instead of an empty screen.
        return vec![Crate {
            name: "Licenses".to_string(),
            version: String::new(),
            licenses: vec![License {
                name: "Licenses".to_string(),
                text: text.trim().to_string(),
            }],
        }];
    }

    crates
        .into_iter()
        .map(|((name, version), licenses)| Crate {
            name,
            version,
            licenses,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\n@@ MIT License\nUsed by:\n - a 1.0.0\n - b 2.0.0\n\nMIT text\n\n@@ Apache License 2.0\nUsed by:\n - a 1.0.0\n\nApache text\n\n@@ MIT License\nUsed by:\n - a 1.0.0\n\nMIT text\n";

    #[test]
    fn a_crate_collects_the_licenses_of_every_section_it_appears_in() {
        let crates = parse(SAMPLE);
        assert_eq!(crates.len(), 2);
        assert_eq!(crates[0].name, "a");
        assert_eq!(crates[0].version, "1.0.0");
        assert_eq!(crates[0].licenses.len(), 2);
        assert_eq!(crates[1].name, "b");
        assert_eq!(crates[1].licenses.len(), 1);
    }

    #[test]
    fn the_same_license_text_is_listed_once_per_crate() {
        let crates = parse(SAMPLE);
        assert_eq!(crates[0].license_names(), "MIT License, Apache License 2.0");
        assert!(crates[0].licenses[0].text.contains("MIT text"));
        assert!(!crates[0].licenses[0].text.contains("Used by"));
    }

    #[test]
    fn crlf_line_endings_parse_the_same() {
        let crates = parse(&SAMPLE.replace('\n', "\r\n"));
        assert_eq!(crates.len(), 2);
        assert_eq!(crates[0].licenses.len(), 2);
        assert_eq!(crates[0].licenses[0].text, "MIT text");
    }

    #[test]
    fn a_crate_row_survives_a_versionless_entry() {
        let crates = parse("\n@@ MIT License\nUsed by:\n - lone\n\nMIT text");
        assert_eq!(crates[0].name, "lone");
        assert_eq!(crates[0].version, "");
    }

    #[test]
    fn long_lines_are_wrapped_to_the_dialog_width() {
        let text = format!("short\n{}\n", "x".repeat(WRAP * 2 + 1));
        let lines = wrap(&text);
        assert_eq!(lines[0], "short");
        assert_eq!(lines[1].chars().count(), WRAP);
        assert_eq!(lines[2].chars().count(), WRAP);
        assert_eq!(lines[3].chars().count(), 1);
    }

    #[test]
    fn wrapping_counts_characters_not_bytes() {
        let lines = wrap(&"“".repeat(WRAP + 1));
        assert_eq!(lines[0].chars().count(), WRAP);
        assert_eq!(lines[1], "“");
    }

    #[test]
    fn a_wrapped_line_breaks_between_words() {
        let text = "alpha ".repeat(20) + "omega";
        let lines = wrap(&text);
        assert!(lines.iter().all(|line| line.chars().count() <= WRAP));
        assert!(lines[0].ends_with("alpha"));
        assert_eq!(lines.concat().replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn a_file_without_sections_is_shown_whole() {
        let crates = parse("just some text\n");
        assert_eq!(crates.len(), 1);
        assert_eq!(crates[0].licenses[0].text, "just some text");
    }

    #[test]
    fn the_bundled_list_parses_into_crates() {
        let crates = crates();
        assert!(crates.len() > 100, "{} crates", crates.len());
        assert!(crates.iter().all(|krate| !krate.name.is_empty()));
        assert!(crates.iter().all(|krate| !krate.licenses.is_empty()));
        // Every row has to fit the dialog, or the text is clipped.
        for krate in crates {
            for line in krate.lines() {
                assert!(
                    line.text.chars().count() <= WRAP,
                    "{}: {:?}",
                    krate.name,
                    line.text
                );
            }
        }
    }
}
