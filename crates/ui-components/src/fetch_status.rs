//! Indicators for ongoing fetches and section-level recovery.

use gpui_kit::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, TestSupportExt, Window,
    assets::IconName,
    base::TestTextExt,
    component::{
        Sizable,
        alert::Alert,
        button::Button,
        empty::{Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle},
        progress::Progress,
        tag::Tag,
    },
    div, px,
};

/// The indeterminate network loader, sized to fit beneath the titlebar.
pub fn fetch_loader() -> impl IntoElement {
    div().id("fetch-loader").test_support().flex_none().child(
        Progress::new("fetch-progress")
            .with_size(px(2.))
            .loading(true)
            .accessibility_label("Loading network requests"),
    )
}

/// The titlebar indicator shown while AniList requests are waiting.
pub fn rate_limit_chip(seconds: u64) -> impl IntoElement {
    let label = SharedString::from(format!("AniList rate limit · {seconds}s"));
    div()
        .id("anilist-rate-limit")
        .test_support()
        .flex_none()
        .mr_2()
        .child(
            Tag::secondary()
                .small()
                .rounded_full()
                .child(label.clone().test_text(label)),
        )
}

/// The user-facing failure to show in an error card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchError<'a> {
    /// A request failed with a displayable explanation.
    Request(&'a str),
    /// AniList returned 403 and is temporarily unavailable.
    AniListUnavailable,
}

/// An inline failure with a retry action. Supply a section-local element ID.
pub fn error_card(
    id: impl Into<ElementId>,
    error: FetchError<'_>,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (title, detail) = match error {
        FetchError::Request(detail) => ("Couldn’t load this section", detail),
        FetchError::AniListUnavailable => ("AniList unavailable", "Try again in a moment."),
    };
    recovery_card(id, title, detail, retry)
}

/// A banner accompanying cached content, with a human-readable cache age.
pub fn offline_banner(last_updated: &str) -> impl IntoElement {
    let label = SharedString::from(format!("Offline: last updated {last_updated}"));
    div().id("offline-banner").test_support().child(
        Alert::new("offline-alert", label.clone())
            .icon(IconName::WifiOff)
            .small()
            .test_text(label),
    )
}

/// A retryable empty state for a section without a cached copy.
pub fn offline_empty(
    id: impl Into<ElementId>,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    recovery_card(
        id,
        "You’re offline",
        "There’s no saved content for this section.",
        retry,
    )
}

fn recovery_card(
    id: impl Into<ElementId>,
    title: &str,
    detail: &str,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let title = SharedString::from(title.to_owned());
    let detail = SharedString::from(detail.to_owned());
    div().id(id).test_support().child(
        Empty::new()
            .border_1()
            .header(
                EmptyHeader::new()
                    .title(EmptyTitle::new().child(title.clone().test_text(title)))
                    .description(EmptyDescription::new().child(detail.clone().test_text(detail))),
            )
            .content(
                EmptyContent::new().child(
                    Button::new("fetch-retry")
                        .small()
                        .outline()
                        .label("Retry")
                        .on_click(retry),
                ),
            ),
    )
}
