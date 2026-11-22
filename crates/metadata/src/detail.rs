//! The media detail query behind the Anime Entry page.

use crate::{
    AniList, Character, CharacterRole, Error, MediaDetail, MediaFormat, MediaRelation, MediaSeason,
    MediaStatus, MediaSummary, Relation, VoiceActor,
    fragments::{CoverImage, Date, MediaCard, MediaType, Title, count},
    schema,
};

/// Characters fetched with a detail: enough for the Entry page's section.
const CHARACTERS_PER_PAGE: i32 = 25;
/// Recommendations fetched with a detail, best rated first.
const RECOMMENDATIONS_PER_PAGE: i32 = 25;

impl AniList {
    /// Fetch everything the Anime Entry page shows about one media, in one
    /// request: header fields, synopsis, cover and banner, main studios,
    /// characters with Japanese voice actors, relations and recommendations.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when AniList has no media with `anilist_id`,
    /// [`Error::Net`] when the request fails (including AniList's rate limit
    /// and outages), [`Error::Api`] for another GraphQL error, or
    /// [`Error::Decode`] for an unexpected response.
    pub async fn media(&self, anilist_id: u32) -> Result<MediaDetail, Error> {
        use cynic::QueryBuilder;

        let id = i32::try_from(anilist_id).map_err(|_| Error::NotFound)?;
        let operation = DetailQuery::build(DetailVariables {
            id,
            characters_per_page: CHARACTERS_PER_PAGE,
            recommendations_per_page: RECOMMENDATIONS_PER_PAGE,
        });
        let media = self.query(operation).await?.media.ok_or(Error::NotFound)?;
        Ok(media.into())
    }
}

#[derive(cynic::QueryVariables, Debug)]
struct DetailVariables {
    id: i32,
    characters_per_page: i32,
    recommendations_per_page: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "DetailVariables")]
struct DetailQuery {
    #[cynic(rename = "Media")]
    #[arguments(id: $id)]
    media: Option<DetailMedia>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Media", variables = "DetailVariables")]
struct DetailMedia {
    // The card fields again: AniList rejects the untyped inline fragment
    // (`... { }`) that `#[cynic(spread)]` of `MediaCard` would emit.
    id: i32,
    id_mal: Option<i32>,
    #[cynic(rename = "type")]
    kind: Option<MediaType>,
    title: Option<Title>,
    cover_image: Option<CoverImage>,
    banner_image: Option<String>,
    format: Option<MediaFormat>,
    #[arguments(version: 2)]
    status: Option<MediaStatus>,
    season: Option<MediaSeason>,
    season_year: Option<i32>,
    episodes: Option<i32>,
    chapters: Option<i32>,
    mean_score: Option<i32>,
    genres: Option<Vec<Option<String>>>,
    is_adult: Option<bool>,
    #[arguments(asHtml: false)]
    description: Option<String>,
    start_date: Option<Date>,
    end_date: Option<Date>,
    duration: Option<i32>,
    popularity: Option<i32>,
    #[arguments(isMain: true)]
    studios: Option<StudioConnection>,
    #[arguments(sort: [ROLE, RELEVANCE, ID], perPage: $characters_per_page)]
    characters: Option<CharacterConnection>,
    relations: Option<MediaConnection>,
    #[arguments(sort: [RATING_DESC, ID], perPage: $recommendations_per_page)]
    recommendations: Option<RecommendationConnection>,
}

#[derive(cynic::QueryFragment, Debug)]
struct StudioConnection {
    nodes: Option<Vec<Option<Studio>>>,
}

#[derive(cynic::QueryFragment, Debug)]
struct Studio {
    name: String,
}

#[derive(cynic::QueryFragment, Debug)]
struct CharacterConnection {
    edges: Option<Vec<Option<CharacterEdge>>>,
}

#[derive(cynic::QueryFragment, Debug)]
struct CharacterEdge {
    role: Option<CharacterRole>,
    node: Option<CharacterNode>,
    #[arguments(language: JAPANESE, sort: [RELEVANCE, ID])]
    voice_actors: Option<Vec<Option<Staff>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Character")]
struct CharacterNode {
    id: i32,
    name: Option<CharacterName>,
    image: Option<CharacterImage>,
}

#[derive(cynic::QueryFragment, Debug)]
struct CharacterName {
    full: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
struct CharacterImage {
    large: Option<String>,
    medium: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
struct Staff {
    id: i32,
    name: Option<StaffName>,
    image: Option<StaffImage>,
}

#[derive(cynic::QueryFragment, Debug)]
struct StaffName {
    full: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
struct StaffImage {
    large: Option<String>,
    medium: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
struct MediaConnection {
    edges: Option<Vec<Option<MediaEdge>>>,
}

#[derive(cynic::QueryFragment, Debug)]
struct MediaEdge {
    #[arguments(version: 2)]
    relation_type: Option<MediaRelation>,
    node: Option<MediaCard>,
}

#[derive(cynic::QueryFragment, Debug)]
struct RecommendationConnection {
    nodes: Option<Vec<Option<Recommendation>>>,
}

#[derive(cynic::QueryFragment, Debug)]
struct Recommendation {
    media_recommendation: Option<MediaCard>,
}

/// The present items of a nullable GraphQL list of nullable items.
fn present<T>(list: Option<Vec<Option<T>>>) -> impl Iterator<Item = T> {
    list.into_iter().flatten().flatten()
}

impl From<DetailMedia> for MediaDetail {
    fn from(media: DetailMedia) -> Self {
        let card = MediaCard {
            id: media.id,
            id_mal: media.id_mal,
            kind: media.kind,
            title: media.title,
            cover_image: media.cover_image,
            banner_image: media.banner_image,
            format: media.format,
            status: media.status,
            season: media.season,
            season_year: media.season_year,
            episodes: media.episodes,
            chapters: media.chapters,
            mean_score: media.mean_score,
            genres: media.genres,
            is_adult: media.is_adult,
        };
        Self {
            summary: card.into(),
            description: media.description,
            start_date: media.start_date.map_or_default(Date::into_fuzzy),
            end_date: media.end_date.map_or_default(Date::into_fuzzy),
            duration: media.duration.and_then(count),
            popularity: media.popularity.and_then(count),
            studios: present(media.studios.and_then(|studios| studios.nodes))
                .map(|studio| studio.name)
                .collect(),
            characters: present(media.characters.and_then(|characters| characters.edges))
                .filter_map(Character::from_edge)
                .collect(),
            relations: present(media.relations.and_then(|relations| relations.edges))
                .filter_map(|edge| {
                    Some(Relation {
                        relation: edge.relation_type,
                        media: edge.node?.into(),
                    })
                })
                .collect(),
            recommendations: present(media.recommendations.and_then(|recs| recs.nodes))
                .filter_map(|rec| rec.media_recommendation)
                .map(MediaSummary::from)
                .collect(),
        }
    }
}

impl Character {
    fn from_edge(edge: CharacterEdge) -> Option<Self> {
        let node = edge.node?;
        Some(Self {
            anilist_id: count(node.id)?,
            name: node.name.and_then(|name| name.full),
            image: node.image.and_then(|image| image.large.or(image.medium)),
            role: edge.role,
            voice_actors: present(edge.voice_actors)
                .filter_map(|staff| {
                    Some(VoiceActor {
                        anilist_id: count(staff.id)?,
                        name: staff.name.and_then(|name| name.full),
                        image: staff.image.and_then(|image| image.large.or(image.medium)),
                    })
                })
                .collect(),
        })
    }
}
