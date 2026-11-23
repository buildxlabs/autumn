//! The authenticated `Viewer` query behind account login.

use cynic::QueryBuilder;

use crate::{AniList, Error, schema};

/// The account AniList returns for a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    /// AniList's user id.
    pub id: u32,
    /// The display name.
    pub name: String,
    /// The avatar URLs, largest first where AniList has one.
    pub avatar: Avatar,
}

/// A user's AniList avatar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Avatar {
    /// The large image URL.
    pub large: Option<String>,
    /// The medium image URL.
    pub medium: Option<String>,
}

impl AniList {
    /// Reads the account that owns `token`.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails (including an expired token's
    /// 401), [`Error::Api`] for another GraphQL error or an invalid id, or
    /// [`Error::Decode`] for an unexpected response.
    pub async fn viewer(&self, token: &str) -> Result<Viewer, Error> {
        let data = self.authenticated(ViewerQuery::build(()), token).await?;
        let user = data.viewer.ok_or(Error::Unauthorized)?;
        let id = u32::try_from(user.id).map_err(|_| Error::Api {
            status: None,
            message: "AniList returned an invalid user id".to_owned(),
        })?;
        Ok(Viewer {
            id,
            name: user.name,
            avatar: Avatar {
                large: user.avatar.as_ref().and_then(|avatar| avatar.large.clone()),
                medium: user.avatar.and_then(|avatar| avatar.medium),
            },
        })
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
struct ViewerQuery {
    #[cynic(rename = "Viewer")]
    viewer: Option<User>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "User")]
struct User {
    id: i32,
    name: String,
    avatar: Option<UserAvatar>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "UserAvatar")]
struct UserAvatar {
    large: Option<String>,
    medium: Option<String>,
}
