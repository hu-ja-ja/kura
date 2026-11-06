//! Search and filtering over the item list.
//!
//! Plain functions so the behavior can be checked without a server.

use crate::model::{ItemKind, VaultItem};

/// One active filter. The bar shows one at a time, like the scope of a search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultFilter {
    All,
    Favorites,
    Kind(ItemKind),
    /// A folder id from [`crate::Folder`].
    Folder(String),
}

impl VaultFilter {
    fn matches(&self, item: &VaultItem) -> bool {
        match self {
            VaultFilter::All => true,
            VaultFilter::Favorites => item.favorite,
            VaultFilter::Kind(kind) => item.kind == *kind,
            VaultFilter::Folder(id) => item.folder_id.as_deref() == Some(id.as_str()),
        }
    }
}

/// Indices of the items matching `query` and `filter`, in list order.
pub fn filter_indices(items: &[VaultItem], query: &str, filter: &VaultFilter) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            filter.matches(item) && (query.is_empty() || matches_query(item, &query))
        })
        .map(|(index, _)| index)
        .collect()
}

fn matches_query(item: &VaultItem, query: &str) -> bool {
    item.search.contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Folder, ItemKind};

    fn search_indices(items: &[VaultItem], query: &str) -> Vec<usize> {
        filter_indices(items, query, &VaultFilter::All)
    }

    fn item(name: &str, username: &str, uris: &[&str]) -> VaultItem {
        let uris: Vec<String> = uris.iter().map(|uri| uri.to_string()).collect();
        let mut search = name.to_lowercase();
        for part in std::iter::once(username).chain(uris.iter().map(String::as_str)) {
            search.push('\0');
            search.push_str(&part.to_lowercase());
        }
        VaultItem {
            id: name.to_string(),
            name: name.to_string(),
            subtitle: username.to_string(),
            username: username.to_string(),
            uris,
            search,
            kind: ItemKind::Login,
            favorite: false,
            folder_id: None,
        }
    }

    fn items() -> Vec<VaultItem> {
        let mut favourite = item("Favourite", "me@example.com", &[]);
        favourite.favorite = true;
        favourite.folder_id = Some("folder-work".to_string());

        let mut bank = item(
            "銀行",
            "user@example.com",
            &["https://my-bank.example.jp/login"],
        );
        bank.kind = ItemKind::Card;
        bank.folder_id = Some("folder-money".to_string());

        vec![
            item("GitHub", "octocat", &["https://github.com"]),
            bank,
            item("Docker Hub", "", &["https://hub.docker.com"]),
            favourite,
        ]
    }

    #[test]
    fn empty_query_returns_everything() {
        assert_eq!(search_indices(&items(), "   ").len(), 4);
    }

    #[test]
    fn matches_name_username_and_uri() {
        let items = items();
        assert_eq!(search_indices(&items, "github"), vec![0]);
        assert_eq!(search_indices(&items, "octocat"), vec![0]);
        assert_eq!(search_indices(&items, "hub.docker"), vec![2]);
    }

    #[test]
    fn ignores_case_and_surrounding_space() {
        let items = items();
        assert_eq!(search_indices(&items, "  DoCkEr  "), vec![2]);
    }

    #[test]
    fn matches_japanese() {
        let items = items();
        assert_eq!(search_indices(&items, "銀行"), vec![1]);
        assert_eq!(search_indices(&items, "example.jp"), vec![1]);
    }

    #[test]
    fn a_query_cannot_match_across_fields() {
        let items = items();
        assert!(search_indices(&items, "github\noctocat").is_empty());
    }

    #[test]
    fn no_match_returns_nothing() {
        assert!(search_indices(&items(), "nothing here").is_empty());
    }

    #[test]
    fn handles_five_thousand_items() {
        let items: Vec<VaultItem> = (0..5_000)
            .map(|i| item(&format!("item-{i}"), &format!("user-{i}"), &[]))
            .collect();
        assert_eq!(search_indices(&items, "item-4999"), vec![4_999]);
        assert_eq!(search_indices(&items, "user-1234").len(), 1);
    }

    #[test]
    fn filters_by_favourite() {
        assert_eq!(
            filter_indices(&items(), "", &VaultFilter::Favorites),
            vec![3]
        );
    }

    #[test]
    fn filters_by_kind() {
        assert_eq!(
            filter_indices(&items(), "", &VaultFilter::Kind(ItemKind::Card)),
            vec![1]
        );
        assert_eq!(
            filter_indices(&items(), "", &VaultFilter::Kind(ItemKind::Login)),
            vec![0, 2, 3]
        );
    }

    #[test]
    fn filters_by_folder() {
        assert_eq!(
            filter_indices(&items(), "", &VaultFilter::Folder("folder-work".into())),
            vec![3]
        );
    }

    #[test]
    fn filter_and_query_combine() {
        let items = items();
        assert_eq!(
            filter_indices(&items, "github", &VaultFilter::Kind(ItemKind::Login)),
            vec![0]
        );
        assert!(filter_indices(&items, "github", &VaultFilter::Kind(ItemKind::Card)).is_empty());
    }

    #[test]
    fn folders_have_a_shape() {
        let folder = Folder {
            id: "folder-work".into(),
            name: "Work".into(),
        };
        assert_eq!(folder.id, "folder-work");
    }
}
