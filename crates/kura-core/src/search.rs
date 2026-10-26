//! Search over the item list.
//!
//! Plain functions so the behavior can be checked without a server.

use crate::model::VaultItem;

/// Indices of the items matching `query`, in list order.
pub fn search_indices(items: &[VaultItem], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..items.len()).collect();
    }

    items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches(item, &query))
        .map(|(index, _)| index)
        .collect()
}

fn matches(item: &VaultItem, query: &str) -> bool {
    item.name.to_lowercase().contains(query)
        || item.username.to_lowercase().contains(query)
        || item
            .uris
            .iter()
            .any(|uri| uri.to_lowercase().contains(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemKind;

    fn item(name: &str, username: &str, uris: &[&str]) -> VaultItem {
        VaultItem {
            id: name.to_string(),
            name: name.to_string(),
            subtitle: username.to_string(),
            username: username.to_string(),
            uris: uris.iter().map(|uri| uri.to_string()).collect(),
            kind: ItemKind::Login,
            favorite: false,
        }
    }

    fn items() -> Vec<VaultItem> {
        vec![
            item("GitHub", "octocat", &["https://github.com"]),
            item(
                "銀行",
                "user@example.com",
                &["https://my-bank.example.jp/login"],
            ),
            item("Docker Hub", "", &["https://hub.docker.com"]),
        ]
    }

    #[test]
    fn empty_query_returns_everything() {
        assert_eq!(search_indices(&items(), "   ").len(), 3);
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
}
