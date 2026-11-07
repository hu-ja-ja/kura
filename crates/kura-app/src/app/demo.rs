//! Sample data and offscreen capture for the README screenshot.
//!
//! Debug builds only. `--demo` opens the window with a fictional vault instead
//! of the user's settings and saved login; `--screenshot=<path>` captures the
//! client area and quits.

#[cfg(feature = "screenshots")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
#[cfg(feature = "screenshots")]
use std::time::Duration;

use gpui_kit::component::IndexPath;
use gpui_kit::*;
use kura_core::{Folder, ItemKind, TotpCode, VaultItem, VaultItemDetail, VaultSnapshot};
use zeroize::Zeroizing;

use crate::settings::{Settings, ThemeChoice, WindowPlacement};

use super::Root;

const WIDTH: f32 = 1200.;
const HEIGHT: f32 = 675.;

/// The flags that turned the demo on. Set once, before any window exists.
static OPTIONS: OnceLock<Options> = OnceLock::new();

pub(crate) struct Options {
    lang: String,
    screenshot: Option<PathBuf>,
}

impl Options {
    pub(crate) fn language(&self) -> &str {
        &self.lang
    }
}

/// Read `--demo` and its companions. `None` for a normal launch.
pub(crate) fn install() -> Option<&'static Options> {
    let mut demo = false;
    let mut lang = None;
    let mut screenshot = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--demo" => demo = true,
            _ => {
                if let Some(value) = arg.strip_prefix("--lang=") {
                    lang = Some(value.to_string());
                } else if let Some(value) = arg.strip_prefix("--screenshot=") {
                    screenshot = Some(PathBuf::from(value));
                }
            }
        }
    }
    demo.then(|| {
        OPTIONS.get_or_init(|| Options {
            lang: lang.unwrap_or_else(|| "en".to_string()),
            screenshot,
        })
    })
}

/// The installed demo options, for the controller's later branches.
pub(crate) fn active() -> Option<&'static Options> {
    OPTIONS.get()
}

/// Settings that keep the demo away from the real profile: dark theme, no
/// capture protection, and a fixed window size for reproducible images.
pub(crate) fn settings(options: &Options) -> Settings {
    Settings {
        language: Some(options.language().to_string()),
        theme: ThemeChoice::Dark,
        screenshot_protection: false,
        start_in_tray: false,
        window: Some(WindowPlacement {
            x: 100.,
            y: 100.,
            width: WIDTH,
            height: HEIGHT,
            maximized: false,
        }),
        ..Settings::default()
    }
}

/// Fill the window with the sample vault and open GitHub's detail dialog.
pub(crate) fn schedule(root: &mut Root, window: &mut Window, cx: &mut Context<Root>) {
    let Some(options) = active() else {
        return;
    };
    let (snapshot, detail) = sample(options.language());
    root.seed_demo(snapshot, detail, window, cx);
}

/// Wait for the seeded frame, write it, and quit.
#[cfg(feature = "screenshots")]
pub(crate) fn capture(handle: AnyWindowHandle, cx: &mut AsyncApp) {
    let Some(path) = active().and_then(|options| options.screenshot.clone()) else {
        return;
    };
    cx.spawn(async move |cx| {
        // Give the window time to draw the seeded list and the open dialog.
        cx.background_executor()
            .timer(Duration::from_millis(800))
            .await;
        match handle.update(cx, |_, window, _| window.render_to_image()) {
            Ok(Ok(image)) => save(&image, &path),
            Ok(Err(err)) => log::error!("the screenshot could not be rendered: {err}"),
            Err(err) => log::error!("the screenshot window is gone: {err}"),
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Without the feature the window still opens, but there is no image.
#[cfg(not(feature = "screenshots"))]
pub(crate) fn capture(_: AnyWindowHandle, _: &mut AsyncApp) {
    if let Some(path) = active().and_then(|options| options.screenshot.as_ref()) {
        log::error!(
            "cannot write {}: rebuild with --features screenshots",
            path.display()
        );
    }
}

#[cfg(feature = "screenshots")]
fn save(image: &image::RgbaImage, path: &Path) {
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        log::error!("could not create {}: {err}", parent.display());
        return;
    }
    if let Err(err) = image.save(path) {
        log::error!("could not write {}: {err}", path.display());
    }
}

impl Root {
    fn seed_demo(
        &mut self,
        snapshot: VaultSnapshot,
        detail: VaultItemDetail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let notes = detail
            .notes
            .as_ref()
            .map(|notes| notes.to_string())
            .unwrap_or_default();
        let snapshot = Arc::new(snapshot);
        self.page = super::Page::Vault;
        self.persist = false;
        self.snapshot = snapshot.clone();
        self.busy = false;
        self.settings_open = false;
        self.filter = kura_core::VaultFilter::All;
        self.list.update(cx, |list, cx| {
            list.delegate_mut().set_snapshot(snapshot);
            list.delegate_mut().set_loading(false);
            list.set_query("", window, cx);
            list.set_selected_index(Some(IndexPath::new(0)), window, cx);
            cx.notify();
        });
        self.detail.selected_id = Some(detail.id.clone());
        self.detail.loading = false;
        self.detail.problem = None;
        self.detail.generation += 1;
        self.detail.rows = Arc::new(crate::detail::rows(&detail));
        self.detail.item = Some(detail);
        self.detail
            .notes
            .update(cx, |state, cx| state.set_value(notes, window, cx));
        self.search
            .update(cx, |search, cx| search.set_value("", window, cx));
        cx.notify();
    }
}

fn sample(lang: &str) -> (VaultSnapshot, VaultItemDetail) {
    let ja = lang == "ja";
    let folders = vec![
        folder("work", if ja { "仕事" } else { "Work" }),
        folder("personal", if ja { "個人" } else { "Personal" }),
        folder("finance", if ja { "金融" } else { "Finance" }),
        folder("development", if ja { "開発" } else { "Development" }),
    ];
    let items = vec![
        login(
            "github",
            "GitHub",
            "haru.dev",
            &["https://github.com/login", "https://gist.github.com"],
            Some("development"),
            true,
        ),
        login(
            "cloudflare",
            "Cloudflare",
            "haru@example.com",
            &["https://dash.cloudflare.com/login"],
            Some("development"),
            false,
        ),
        login(
            "aws",
            "AWS Console",
            "platform-admin",
            &["https://console.aws.amazon.com"],
            Some("work"),
            true,
        ),
        login(
            "figma",
            "Figma",
            "haru@example.com",
            &["https://www.figma.com/login"],
            Some("work"),
            false,
        ),
        login(
            "notion",
            "Notion",
            "haru@example.com",
            &["https://www.notion.so/login"],
            Some("work"),
            false,
        ),
        login(
            "slack",
            "Slack",
            "haru@example.com",
            &["https://slack.com/signin"],
            Some("work"),
            false,
        ),
        login(
            "google",
            "Google",
            "haru@example.com",
            &["https://accounts.google.com"],
            Some("personal"),
            false,
        ),
        login(
            "amazon",
            "Amazon",
            "haru@example.com",
            &["https://www.amazon.co.jp"],
            Some("personal"),
            false,
        ),
        login(
            "dropbox",
            "Dropbox",
            "haru@example.com",
            &["https://www.dropbox.com/login"],
            Some("personal"),
            false,
        ),
        login(
            "steam",
            "Steam",
            "haru_plays",
            &["https://store.steampowered.com/login"],
            Some("personal"),
            false,
        ),
        plain(
            "visa",
            "Visa",
            "Visa",
            ItemKind::Card,
            Some("finance"),
            true,
        ),
        plain(
            "drivers-license",
            if ja {
                "運転免許証"
            } else {
                "Driver's License"
            },
            if ja {
                "運転免許証"
            } else {
                "Driver's License"
            },
            ItemKind::Identity,
            Some("personal"),
            false,
        ),
        plain(
            "ssh-production",
            if ja {
                "SSH: 本番サーバー"
            } else {
                "SSH: Production server"
            },
            if ja {
                "本番サーバー"
            } else {
                "Production server"
            },
            ItemKind::SshKey,
            Some("development"),
            false,
        ),
        plain(
            "wifi-home",
            if ja { "Wi-Fi: 自宅" } else { "Wi-Fi: Home" },
            if ja {
                "自宅ネットワーク"
            } else {
                "Home network"
            },
            ItemKind::SecureNote,
            Some("personal"),
            false,
        ),
    ];

    let detail = VaultItemDetail {
        id: "github".to_string(),
        name: "GitHub".to_string(),
        kind: ItemKind::Login,
        favorite: true,
        folder_id: Some("development".to_string()),
        username: Some("haru.dev".to_string()),
        password: Some(Zeroizing::new("kura-demo-passphrase-not-real".to_string())),
        uris: vec![
            "https://github.com/login".to_string(),
            "https://gist.github.com".to_string(),
        ],
        totp: Some(TotpCode {
            code: "482913".to_string(),
            remaining: 24,
        }),
        notes: Some(Zeroizing::new(if ja {
            "個人用アカウント".to_string()
        } else {
            "Personal account".to_string()
        })),
        fields: Vec::new(),
        card: None,
        identity: None,
        ssh_key: None,
        attachments: Vec::new(),
    };

    (
        VaultSnapshot {
            items,
            folders,
            decrypt_failures: 0,
        },
        detail,
    )
}

fn folder(id: &str, name: &str) -> Folder {
    Folder {
        id: id.to_string(),
        name: name.to_string(),
    }
}

fn login(
    id: &str,
    name: &str,
    username: &str,
    uris: &[&str],
    folder: Option<&str>,
    favorite: bool,
) -> VaultItem {
    let uris: Vec<String> = uris.iter().map(|uri| uri.to_string()).collect();
    item(
        id,
        name,
        username,
        username,
        uris,
        ItemKind::Login,
        favorite,
        folder,
    )
}

fn plain(
    id: &str,
    name: &str,
    subtitle: &str,
    kind: ItemKind,
    folder: Option<&str>,
    favorite: bool,
) -> VaultItem {
    item(id, name, subtitle, "", Vec::new(), kind, favorite, folder)
}

#[allow(clippy::too_many_arguments)]
fn item(
    id: &str,
    name: &str,
    subtitle: &str,
    username: &str,
    uris: Vec<String>,
    kind: ItemKind,
    favorite: bool,
    folder: Option<&str>,
) -> VaultItem {
    let mut search = name.to_lowercase();
    for part in std::iter::once(username).chain(uris.iter().map(String::as_str)) {
        search.push('\0');
        search.push_str(&part.to_lowercase());
    }
    VaultItem {
        id: id.to_string(),
        name: name.to_string(),
        subtitle: subtitle.to_string(),
        username: username.to_string(),
        uris,
        search,
        kind,
        favorite,
        folder_id: folder.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::sample;

    #[test]
    fn the_sample_vault_starts_with_the_open_item() {
        let (snapshot, detail) = sample("en");
        assert_eq!(snapshot.items.len(), 14);
        assert_eq!(snapshot.items[0].id, detail.id);
        assert_eq!(detail.folder_id.as_deref(), Some("development"));
        assert!(
            snapshot
                .folders
                .iter()
                .any(|folder| folder.id == "development" && folder.name == "Development")
        );
    }

    #[test]
    fn the_japanese_sample_localizes_the_folders_and_items() {
        let (snapshot, _) = sample("ja");
        assert!(snapshot.folders.iter().any(|folder| folder.name == "開発"));
        assert!(snapshot.items.iter().any(|item| item.name == "運転免許証"));
    }
}
