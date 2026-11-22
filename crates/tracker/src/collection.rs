use domain::{ListEntry, ListStatus};

/// The order status lists appear in.
const STATUS_ORDER: [ListStatus; 6] = [
    ListStatus::Current,
    ListStatus::Repeating,
    ListStatus::Planning,
    ListStatus::Completed,
    ListStatus::Paused,
    ListStatus::Dropped,
];

/// A user's list for one media kind, as lists of entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Collection {
    /// Non-empty status lists in a fixed order (Current, Repeating, Planning,
    /// Completed, Paused, Dropped), then non-empty custom lists by name. An
    /// entry in custom lists appears in each of them, and also in its status
    /// list unless it is hidden from status lists.
    pub lists: Vec<List>,
}

/// One list in a [`Collection`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// Which list this is.
    pub name: ListName,
    /// Its entries, most recently updated first.
    pub entries: Vec<ListEntry>,
}

/// What a [`List`] gathers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ListName {
    /// Every entry with this status.
    Status(ListStatus),
    /// Every entry in the custom list of this name.
    Custom(String),
}

impl Collection {
    /// Groups `entries`, which are already ordered most recently updated
    /// first.
    pub(crate) fn group(entries: &[ListEntry]) -> Self {
        let mut lists: Vec<List> = STATUS_ORDER
            .iter()
            .map(|&status| List {
                name: ListName::Status(status),
                entries: entries
                    .iter()
                    .filter(|entry| entry.status == status && !entry.hidden_from_status_lists)
                    .cloned()
                    .collect(),
            })
            .collect();
        let mut names: Vec<&String> = entries
            .iter()
            .flat_map(|entry| &entry.custom_lists)
            .collect();
        names.sort();
        names.dedup();
        lists.extend(names.into_iter().map(|name| {
            List {
                name: ListName::Custom(name.clone()),
                entries: entries
                    .iter()
                    .filter(|entry| entry.custom_lists.contains(name))
                    .cloned()
                    .collect(),
            }
        }));
        lists.retain(|list| !list.entries.is_empty());
        Self { lists }
    }
}
