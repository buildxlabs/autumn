//! Authenticated AniList collection reads and list-entry mutations.
//!
//! The collection comes from `MediaListCollection`, which AniList pre-groups
//! by status and custom list. Every group is read: entries hidden from status
//! lists live only in their custom groups. AniList caps the response at the
//! 11,000 most recently updated entries; [`ListCollection::has_next_chunk`]
//! reports that truncation instead of losing it silently.
//!
//! Mutations go through `net::Client::mutate`, which sends each one exactly
//! once. A failed edit is returned to the caller so the UI can offer Retry.

use cynic::{MutationBuilder, QueryBuilder};
use domain::{FuzzyDate, ListStatus, MediaKind};

use crate::{
    Error,
    client::AniList,
    fragments::{Date, MediaType, count},
    schema,
};

/// The account's score display format (AniList `ScoreFormat`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum ScoreFormat {
    /// An integer from 0 to 100.
    #[cynic(rename = "POINT_100")]
    Point100,
    /// A float from 0 to 10 with one decimal.
    #[cynic(rename = "POINT_10_DECIMAL")]
    Point10Decimal,
    /// An integer from 0 to 10.
    #[cynic(rename = "POINT_10")]
    Point10,
    /// Stars, from 0 to 5.
    #[cynic(rename = "POINT_5")]
    Point5,
    /// Smileys, from 0 to 3.
    #[cynic(rename = "POINT_3")]
    Point3,
    /// A format this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}

/// One AniList list group, status or custom list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListGroup {
    /// The group's name: a status or a custom-list name.
    pub name: String,
    /// Whether this is a custom-list group.
    pub custom: bool,
    /// The entries in the group.
    pub entries: Vec<ListEntryData>,
}

/// All custom-list names on an account, including lists with no entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomListNames {
    /// Names configured on the anime list.
    pub anime: Vec<String>,
    /// Names configured on the manga list.
    pub manga: Vec<String>,
}

/// One list entry as AniList stores it, before `MediaId` minting.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListEntryData {
    /// The AniList list-entry id, needed to delete or update this entry.
    pub id: i32,
    /// The AniList media id, used to find or mint a [`domain::MediaId`].
    pub media_id: u32,
    /// Where the media sits in the list.
    pub status: ListStatus,
    /// The score on AniList's raw 0–100 scale; 0 means unscored.
    pub score_raw: u8,
    /// Episodes watched or chapters read.
    pub progress: u32,
    /// Volumes read, for manga.
    pub progress_volumes: u32,
    /// Rewatches or rereads.
    pub repeat: u32,
    /// Priority for planned media.
    pub priority: u32,
    /// Whether the entry is private.
    pub private: bool,
    /// Free-text notes.
    pub notes: String,
    /// Whether this entry is hidden from its status list.
    pub hidden_from_status_lists: bool,
    /// Custom-list names this entry belongs to.
    pub custom_lists: Vec<String>,
    /// When the media was started.
    pub started_at: FuzzyDate,
    /// When the media was completed.
    pub completed_at: FuzzyDate,
}

/// A collection with the account's score format.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListCollection {
    /// Status and custom-list groups, including entries hidden from status
    /// lists.
    pub lists: Vec<ListGroup>,
    /// How the account displays scores.
    pub score_format: ScoreFormat,
    /// Whether AniList stopped at its 11,000-entry cap.
    pub has_next_chunk: bool,
    /// Every configured custom-list name, including empty lists.
    pub custom_lists: CustomListNames,
}

/// The writable fields of a list entry. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "callers build mutations from these optional fields"
)]
pub struct ListEntryInput {
    /// The existing list-entry id, when updating.
    pub id: Option<i32>,
    /// The AniList media id, required when creating an entry.
    pub media_id: Option<u32>,
    /// A new status.
    pub status: Option<ListStatus>,
    /// A new raw 0–100 score.
    pub score_raw: Option<u8>,
    /// New progress.
    pub progress: Option<u32>,
    /// New volume progress.
    pub progress_volumes: Option<u32>,
    /// A new rewatch count.
    pub repeat: Option<u32>,
    /// A new priority.
    pub priority: Option<u32>,
    /// A new private flag.
    pub private: Option<bool>,
    /// New notes.
    pub notes: Option<String>,
    /// A new hidden-from-status-lists flag.
    pub hidden_from_status_lists: Option<bool>,
    /// Custom-list names that should be enabled for this entry.
    pub custom_lists: Option<Vec<String>>,
    /// A new start date.
    pub started_at: Option<FuzzyDate>,
    /// A new completion date.
    pub completed_at: Option<FuzzyDate>,
}

#[derive(cynic::QueryVariables, Debug)]
struct CollectionVariables {
    user_name: String,
    kind: MediaType,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "CollectionVariables")]
struct CollectionQuery {
    #[cynic(rename = "MediaListCollection")]
    #[arguments(userName: $user_name, type: $kind, sort: [UPDATED_TIME_DESC])]
    collection: Option<Collection>,
    #[cynic(rename = "Viewer")]
    viewer: Option<Viewer>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaListCollection")]
struct Collection {
    lists: Option<Vec<Option<Group>>>,
    has_next_chunk: Option<bool>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaListGroup")]
struct Group {
    name: Option<String>,
    is_custom_list: Option<bool>,
    entries: Option<Vec<Option<Entry>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaList")]
struct Entry {
    id: i32,
    media_id: i32,
    status: Option<MediaListStatus>,
    #[arguments(format: POINT_100)]
    score: Option<f64>,
    progress: Option<i32>,
    progress_volumes: Option<i32>,
    repeat: Option<i32>,
    priority: Option<i32>,
    private: Option<bool>,
    notes: Option<String>,
    hidden_from_status_lists: Option<bool>,
    started_at: Option<Date>,
    completed_at: Option<Date>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "User")]
struct Viewer {
    media_list_options: Option<ListOptions>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaListOptions")]
struct ListOptions {
    score_format: Option<ScoreFormat>,
    anime_list: Option<TypeOptions>,
    manga_list: Option<TypeOptions>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaListTypeOptions")]
struct TypeOptions {
    custom_lists: Option<Vec<Option<String>>>,
}

#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
enum MediaListStatus {
    Current,
    Planning,
    Completed,
    Dropped,
    Paused,
    Repeating,
    #[cynic(fallback)]
    Other(String),
}

impl From<MediaListStatus> for Option<ListStatus> {
    fn from(status: MediaListStatus) -> Self {
        match status {
            MediaListStatus::Current => Some(ListStatus::Current),
            MediaListStatus::Planning => Some(ListStatus::Planning),
            MediaListStatus::Completed => Some(ListStatus::Completed),
            MediaListStatus::Dropped => Some(ListStatus::Dropped),
            MediaListStatus::Paused => Some(ListStatus::Paused),
            MediaListStatus::Repeating => Some(ListStatus::Repeating),
            MediaListStatus::Other(_) => None,
        }
    }
}

fn raw_score(score: Option<f64>) -> u8 {
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "AniList's score(format: POINT_100) is rounded and range-checked first"
    )]
    fn as_u8(value: f64) -> u8 {
        value as u8
    }
    score
        .and_then(|value| {
            let rounded = value.round();
            (0.0..=100.0).contains(&rounded).then(|| as_u8(rounded))
        })
        .unwrap_or(0)
}

impl Entry {
    fn into_data(self, custom_lists: Vec<String>) -> Option<ListEntryData> {
        let status: Option<ListStatus> = self.status?.into();
        Some(ListEntryData {
            id: self.id,
            media_id: count(self.media_id)?,
            status: status?,
            score_raw: raw_score(self.score),
            progress: self.progress.and_then(count).unwrap_or(0),
            progress_volumes: self.progress_volumes.and_then(count).unwrap_or(0),
            repeat: self.repeat.and_then(count).unwrap_or(0),
            priority: self.priority.and_then(count).unwrap_or(0),
            private: self.private.unwrap_or_default(),
            notes: self.notes.unwrap_or_default(),
            hidden_from_status_lists: self.hidden_from_status_lists.unwrap_or_default(),
            started_at: self.started_at.map_or(FuzzyDate::UNKNOWN, Date::into_fuzzy),
            completed_at: self
                .completed_at
                .map_or(FuzzyDate::UNKNOWN, Date::into_fuzzy),
            custom_lists,
        })
    }
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "FuzzyDateInput")]
struct DateInput {
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
}

impl DateInput {
    fn new(date: FuzzyDate) -> Self {
        Self {
            year: date.year().map(i32::from),
            month: date.month().map(i32::from),
            day: date.day().map(i32::from),
        }
    }
}

#[derive(cynic::QueryVariables, Debug)]
struct SaveVariables {
    #[cynic(skip_serializing_if = "Option::is_none")]
    id: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    media_id: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    status: Option<MediaListStatus>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    score_raw: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    progress: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    progress_volumes: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    repeat: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    private: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    hidden: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    custom_lists: Option<Vec<String>>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    started_at: Option<DateInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    completed_at: Option<DateInput>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "SaveVariables")]
struct SaveMutation {
    #[cynic(rename = "SaveMediaListEntry")]
    #[arguments(
        id: $id, mediaId: $media_id, status: $status, scoreRaw: $score_raw,
        progress: $progress, progressVolumes: $progress_volumes, repeat: $repeat,
        priority: $priority, private: $private, notes: $notes,
        hiddenFromStatusLists: $hidden, customLists: $custom_lists,
        startedAt: $started_at, completedAt: $completed_at
    )]
    entry: Option<SavedEntry>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaList")]
struct SavedEntry {
    id: i32,
}

#[derive(cynic::QueryVariables, Debug)]
struct DeleteVariables {
    id: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "DeleteVariables")]
struct DeleteMutation {
    #[cynic(rename = "DeleteMediaListEntry")]
    #[arguments(id: $id)]
    deleted: Option<Deleted>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Deleted")]
struct Deleted {
    deleted: Option<bool>,
}

#[derive(cynic::QueryVariables, Debug)]
struct OptionsVariables {
    #[cynic(skip_serializing_if = "Option::is_none")]
    anime: Option<OptionsInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    manga: Option<OptionsInput>,
}

#[derive(cynic::InputObject, Debug)]
#[cynic(graphql_type = "MediaListOptionsInput")]
struct OptionsInput {
    custom_lists: Vec<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "OptionsVariables")]
struct UpdateUserMutation {
    #[cynic(rename = "UpdateUser")]
    #[arguments(animeListOptions: $anime, mangaListOptions: $manga)]
    user: Option<UpdatedUser>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "User")]
struct UpdatedUser {
    #[expect(
        dead_code,
        reason = "the field is selected so the mutation returns a user"
    )]
    id: i32,
}

impl AniList {
    /// Fetches the authenticated user's collection for one media kind.
    ///
    /// Reads every group, including custom lists, and reports AniList's
    /// 11,000-entry cap in [`ListCollection::has_next_chunk`].
    ///
    /// # Errors
    ///
    /// The request's [`Error`] on a network, API or decode failure.
    pub async fn list_collection(
        &self,
        token: &str,
        user_name: &str,
        kind: MediaKind,
    ) -> Result<ListCollection, Error> {
        let Some(kind) = MediaType::from_kind(kind) else {
            return Err(Error::Api {
                status: None,
                message: "unsupported media kind".to_owned(),
            });
        };
        let variables = CollectionVariables {
            user_name: user_name.to_owned(),
            kind,
        };
        let data = self
            .authenticated(CollectionQuery::build(variables), token)
            .await?;
        let viewer = data.viewer;
        let options = viewer
            .as_ref()
            .and_then(|viewer| viewer.media_list_options.as_ref());
        let score_format = options
            .and_then(|options| options.score_format.clone())
            .unwrap_or(ScoreFormat::Point100);
        let names = |kind: Option<&TypeOptions>| -> Vec<String> {
            kind.and_then(|kind| kind.custom_lists.as_ref())
                .map_or_default(|names| names.iter().flatten().cloned().collect())
        };
        let custom_lists = CustomListNames {
            anime: names(options.and_then(|options| options.anime_list.as_ref())),
            manga: names(options.and_then(|options| options.manga_list.as_ref())),
        };
        let collection = data.collection;
        let has_next_chunk = collection
            .as_ref()
            .and_then(|collection| collection.has_next_chunk)
            .unwrap_or(false);
        let mut lists = Vec::new();
        let mut custom = std::collections::HashMap::<i32, Vec<String>>::new();
        for group in collection
            .as_ref()
            .and_then(|c| c.lists.as_ref())
            .into_iter()
            .flatten()
            .flatten()
        {
            if group.is_custom_list.unwrap_or_default()
                && let Some(name) = &group.name
            {
                for entry in group.entries.iter().flatten().flatten() {
                    custom.entry(entry.id).or_default().push(name.clone());
                }
            }
        }
        for group in collection
            .into_iter()
            .filter_map(|c| c.lists)
            .flatten()
            .flatten()
        {
            let is_custom = group.is_custom_list.unwrap_or_default();
            let mut entries = Vec::new();
            for entry in group.entries.into_iter().flatten().flatten() {
                let mut names = custom.get(&entry.id).cloned().unwrap_or_default();
                if is_custom
                    && let Some(name) = &group.name
                    && !names.contains(name)
                {
                    names.push(name.clone());
                }
                if let Some(entry) = entry.into_data(names) {
                    entries.push(entry);
                }
            }
            if let Some(name) = group.name {
                lists.push(ListGroup {
                    name,
                    custom: is_custom,
                    entries,
                });
            }
        }
        Ok(ListCollection {
            lists,
            score_format,
            has_next_chunk,
            custom_lists,
        })
    }

    /// Saves one list entry and returns AniList's list-entry id. Sent once;
    /// no automatic retry, so callers can offer Retry on failure.
    ///
    /// # Errors
    ///
    /// The request's [`Error`] on a network, API or decode failure.
    pub async fn save_list_entry(&self, token: &str, input: &ListEntryInput) -> Result<i32, Error> {
        let status = match input.status {
            Some(status) => Some(match status {
                ListStatus::Current => MediaListStatus::Current,
                ListStatus::Planning => MediaListStatus::Planning,
                ListStatus::Completed => MediaListStatus::Completed,
                ListStatus::Dropped => MediaListStatus::Dropped,
                ListStatus::Paused => MediaListStatus::Paused,
                ListStatus::Repeating => MediaListStatus::Repeating,
                other => {
                    return Err(Error::Api {
                        status: None,
                        message: format!("unsupported list status: {other:?}"),
                    });
                }
            }),
            None => None,
        };
        let variables = SaveVariables {
            id: input.id,
            media_id: input.media_id.and_then(|id| i32::try_from(id).ok()),
            status,
            score_raw: input.score_raw.map(i32::from),
            progress: input.progress.and_then(|value| i32::try_from(value).ok()),
            progress_volumes: input
                .progress_volumes
                .and_then(|value| i32::try_from(value).ok()),
            repeat: input.repeat.and_then(|value| i32::try_from(value).ok()),
            priority: input.priority.and_then(|value| i32::try_from(value).ok()),
            private: input.private,
            notes: input.notes.clone(),
            hidden: input.hidden_from_status_lists,
            custom_lists: input.custom_lists.clone(),
            started_at: input.started_at.map(DateInput::new),
            completed_at: input.completed_at.map(DateInput::new),
        };
        let data = self.mutation(SaveMutation::build(variables), token).await?;
        data.entry.map(|entry| entry.id).ok_or_else(|| Error::Api {
            status: None,
            message: "AniList returned no saved list entry".to_owned(),
        })
    }

    /// Deletes a list entry by its AniList list-entry id. Sent once.
    ///
    /// # Errors
    ///
    /// The request's [`Error`] on a network, API or decode failure.
    pub async fn delete_list_entry(&self, token: &str, id: i32) -> Result<(), Error> {
        let data = self
            .mutation(DeleteMutation::build(DeleteVariables { id }), token)
            .await?;
        if data.deleted.and_then(|deleted| deleted.deleted) == Some(false) {
            return Err(Error::Api {
                status: None,
                message: "AniList did not delete the entry".to_owned(),
            });
        }
        Ok(())
    }

    /// Replaces all custom-list names for `kind` with `names`. AniList has
    /// no dedicated create mutation: `UpdateUser` list options own the names,
    /// and a name used by `SaveMediaListEntry` before it exists there is a
    /// no-op. Pass the full desired list — a subset deletes the other names.
    ///
    /// # Errors
    ///
    /// The request's [`Error`] on a network, API or decode failure.
    pub async fn update_custom_lists(
        &self,
        token: &str,
        kind: MediaKind,
        names: &[String],
    ) -> Result<(), Error> {
        let input = OptionsInput {
            custom_lists: names.to_vec(),
        };
        let variables = OptionsVariables {
            anime: (kind == MediaKind::Anime).then_some(input),
            manga: (kind == MediaKind::Manga).then(|| OptionsInput {
                custom_lists: names.to_vec(),
            }),
        };
        let data = self
            .mutation(UpdateUserMutation::build(variables), token)
            .await?;
        data.user.ok_or_else(|| Error::Api {
            status: None,
            message: "AniList returned no user after UpdateUser".to_owned(),
        })?;
        Ok(())
    }
}
