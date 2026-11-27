//! Query construction: #181 Q1–Q4.

use search::{Indexer, Query, SearchRequest, clean_title, plan};

#[test]
fn cleaning_strips_season_markers_punctuation_and_case() {
    assert_eq!(
        clean_title("Frieren: Beyond Journey's End 2nd Season!"),
        Some("frieren beyond journeys end".to_owned())
    );
    assert_eq!(
        clean_title("[SubsPlease] Bocchi the Rock! S02"),
        Some("subsplease bocchi the rock".to_owned())
    );
    assert_eq!(clean_title("Season 2"), None);
}

#[test]
fn titles_are_ordered_case_insensitively_deduped_and_capped_at_three() {
    let request = SearchRequest::new(
        vec![
            "Frieren: Beyond Journey's End".to_owned(),
            "frieren beyond journeys end".to_owned(),
            "Sousou no Frieren".to_owned(),
            "Frieren 2nd Season".to_owned(),
            "Frieren: The Movie".to_owned(),
        ],
        1,
        5,
    );

    let keywords: Vec<String> = plan(&request)
        .numbered
        .iter()
        .filter_map(|query| match query {
            Query::Nyaa { keyword } => Some(keyword.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        keywords,
        vec![
            "frieren beyond journeys end 05".to_owned(),
            "sousou no frieren 05".to_owned(),
            "frieren 05".to_owned(),
            "frieren beyond journeys end batch".to_owned(),
        ]
    );
}

#[test]
fn absolute_form_is_queried_only_when_it_differs() {
    let same = SearchRequest::new(vec!["One Piece".to_owned()], 1, 2).with_absolute_episode(2);
    assert_eq!(plan(&same).numbered.len(), 4); // single + batch, one per indexer

    let different =
        SearchRequest::new(vec!["One Piece".to_owned()], 1, 2).with_absolute_episode(35);
    let keywords: Vec<String> = plan(&different)
        .numbered
        .iter()
        .filter_map(|query| match query {
            Query::Nyaa { keyword } => Some(keyword.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        keywords,
        vec![
            "one piece 02".to_owned(),
            "one piece 35".to_owned(),
            "one piece batch".to_owned(),
        ]
    );
}

#[test]
fn anime_tosho_numbered_queries_carry_season_and_episode_params() {
    let request = SearchRequest::new(vec!["Frieren".to_owned()], 2, 5).with_absolute_episode(35);
    let numbered: Vec<Query> = plan(&request)
        .numbered
        .into_iter()
        .filter(|query| query.indexer() == Indexer::AnimeTosho)
        .collect();

    assert_eq!(
        numbered,
        vec![
            Query::anime_tosho_keyword("frieren", 2, 5),
            Query::anime_tosho_absolute("frieren", 35),
            Query::anime_tosho_batch("frieren"),
        ]
    );
}

#[test]
fn a_batch_query_is_issued_alongside_the_single_queries() {
    let request = SearchRequest::new(vec!["Frieren".to_owned()], 1, 5);
    let keywords: Vec<String> = plan(&request)
        .numbered
        .iter()
        .filter_map(|query| match query {
            Query::Nyaa { keyword } => Some(keyword.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(keywords, vec!["frieren 05", "frieren batch"]);
}

#[test]
fn a_known_anidb_aid_short_circuits_keyword_variants() {
    let request = SearchRequest::new(
        vec!["Frieren".to_owned(), "Sousou no Frieren".to_owned()],
        1,
        5,
    )
    .with_anidb_ids(17617, Some(271_418));

    let numbered: Vec<Query> = plan(&request)
        .numbered
        .into_iter()
        .filter(|query| query.indexer() == Indexer::AnimeTosho)
        .collect();

    assert_eq!(
        numbered,
        vec![
            Query::anime_tosho_aid(17617),
            Query::anime_tosho_eid(17617, 271_418),
        ]
    );
}

#[test]
fn keyword_fallback_is_last_and_budget_caps_each_indexer_at_eight() {
    let titles = (0..6)
        .map(|n| format!("Title {n}"))
        .collect::<Vec<String>>();
    let request = SearchRequest::new(titles, 1, 5).with_absolute_episode(35);
    let queries = plan(&request);
    let (nyaa, tosho): (Vec<Query>, Vec<Query>) = queries
        .numbered
        .iter()
        .chain(queries.fallback.iter())
        .cloned()
        .partition(|query| query.indexer() == Indexer::Nyaa);

    assert!(nyaa.len() <= 8, "Nyaa over budget: {}", nyaa.len());
    assert!(tosho.len() <= 8, "AnimeTosho over budget: {}", tosho.len());
    assert_eq!(
        queries.fallback.last(),
        Some(&Query::Nyaa {
            keyword: "title 0".to_owned(),
        })
    );
    assert_eq!(
        queries.fallback.first(),
        Some(&Query::anime_tosho_fallback("title 0"))
    );
}
