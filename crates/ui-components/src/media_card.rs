//! The media card: a poster, title, badges, a hover popup with more detail,
//! a context menu and a skeleton while the poster loads.
//! See ARCHITECTURE §2 (ui-components) and the ticket's acceptance criteria.

use std::rc::Rc;

use gpui_kit::{
    App, Div, ElementId, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    Pixels, RenderOnce, Resource, SharedString, Styled, StyledImage, TestSupportExt, Window,
    component::{
        ActiveTheme, Sizable as _,
        hover_card::HoverCard,
        menu::{ContextMenuExt, PopupMenuItem},
        skeleton::Skeleton,
        tag::Tag,
    },
    div, img, px,
};

use crate::POSTER_WIDTH;

/// The poster area's height.
pub const MEDIA_POSTER_HEIGHT: f32 = 240.;
/// The card's height: the poster plus a title row and badge row below it.
pub const MEDIA_CARD_HEIGHT: f32 = MEDIA_POSTER_HEIGHT + 44.;

/// The media kind a card's format badge shows.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaFormat {
    /// An anime: series, film, OVA and the like.
    Anime,
    /// A manga, manhwa, manhua or light novel.
    Manga,
}

impl MediaFormat {
    /// The badge's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Anime => "Anime",
            Self::Manga => "Manga",
        }
    }
}

/// One media as a grid card shows it: plain view-model data; the card
/// renders it.
#[derive(Debug, Clone, Default)]
pub struct MediaCardView {
    /// The poster to show through the grid's image cache. `None` draws the
    /// card's skeleton poster.
    pub poster: Option<Resource>,
    /// The title under the poster.
    pub title: SharedString,
    /// The format badge (anime or manga), if the source reports one.
    pub format: Option<MediaFormat>,
    /// The score as the source shows it, e.g. 8.7; `None` means unscored.
    pub score: Option<f32>,
    /// Episodes watched or chapters read; 0 shows no progress.
    pub progress: u32,
    /// Episodes or chapters the media has, shown beside the progress.
    pub total: Option<u32>,
    /// The detail line in the card's hover popup.
    pub detail: SharedString,
}

/// One media card. Callers set [`MediaCard::width`] to the column width;
/// the context menu offers Open and Add to list, with the matching
/// [`MediaCard::on_open`] / [`MediaCard::on_add_to_list`] callbacks.
#[derive(IntoElement)]
pub struct MediaCard {
    view: MediaCardView,
    id: ElementId,
    width: Pixels,
    add_label: SharedString,
    on_open: Option<CardAction>,
    on_add_to_list: Option<CardAction>,
}

/// An action run when the card's context menu item is clicked.
type CardAction = Rc<dyn Fn(&mut Window, &mut App)>;

impl std::fmt::Debug for MediaCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaCard")
            .field("view", &self.view)
            .field("id", &self.id)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl MediaCard {
    /// A card for `view` with `id`.
    pub fn new(view: MediaCardView, id: impl Into<ElementId>) -> Self {
        Self {
            view,
            id: id.into(),
            width: px(POSTER_WIDTH),
            add_label: "Add to list".into(),
            on_open: None,
            on_add_to_list: None,
        }
    }

    /// Sets the card's width; the grid passes its column width.
    #[must_use]
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Adds the context menu's Open action; clickable when the card is
    /// right-clicked.
    #[must_use]
    pub fn on_open(mut self, on_open: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(on_open));
        self
    }

    /// Renames the context-menu action for entries already on a list.
    #[must_use]
    pub fn add_to_list_label(mut self, label: impl Into<SharedString>) -> Self {
        self.add_label = label.into();
        self
    }

    /// Adds the context menu's Add to list action.
    #[must_use]
    pub fn on_add_to_list(
        mut self,
        on_add_to_list: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_add_to_list = Some(Rc::new(on_add_to_list));
        self
    }
    /// Test-only: fires the card's Open action, as the context menu does.
    #[cfg(test)]
    pub(crate) fn fire_open(&self, window: &mut Window, cx: &mut App) {
        if let Some(on_open) = &self.on_open {
            on_open(window, cx);
        }
    }

    /// Test-only: fires the card's Add to list action.
    #[cfg(test)]
    pub(crate) fn fire_add_to_list(&self, window: &mut Window, cx: &mut App) {
        if let Some(on_add_to_list) = &self.on_add_to_list {
            on_add_to_list(window, cx);
        }
    }
}

impl RenderOnce for MediaCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let MediaCard {
            view,
            id,
            width,
            add_label,
            on_open,
            on_add_to_list,
        } = self;
        let colors = cx.theme().colors;
        let poster_area = match &view.poster {
            Some(poster) => div()
                .debug_selector(|| "media-card-poster".into())
                .w(width)
                .child(
                    img(ImageSource::Resource(poster.clone()))
                        .w_full()
                        .h(px(MEDIA_POSTER_HEIGHT))
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| poster_skeleton().into_any_element())
                        .with_fallback(|| poster_skeleton().into_any_element()),
                ),
            None => div()
                .debug_selector(|| "media-card-skeleton".into())
                .w(width)
                .child(poster_skeleton()),
        };
        let title = title_line(view.title.clone())
            .debug_selector(|| "media-card-title".into())
            .flex_none()
            .h(px(20.))
            .text_color(colors.foreground);
        let card = div()
            .id(id.clone())
            .test_support()
            .flex_col()
            .gap_1()
            .w(width)
            .h(px(MEDIA_CARD_HEIGHT))
            .overflow_hidden()
            .child(poster_area)
            .child(title)
            .children(badge_elements(&view));

        let menu_add_label = add_label;
        HoverCard::new(id)
            .trigger(card.context_menu(move |mut menu, _, _| {
                if let Some(on_open) = on_open.clone() {
                    menu = menu.item(PopupMenuItem::new("Open").on_click(move |_, window, cx| {
                        on_open(window, cx);
                    }));
                }
                if let Some(on_add_to_list) = on_add_to_list.clone() {
                    menu = menu.item(
                        PopupMenuItem::new(menu_add_label.clone())
                            .on_click(move |_, window, cx| on_add_to_list(window, cx)),
                    );
                }
                menu
            }))
            .content(move |_state, _window, cx| {
                let colors = cx.theme().colors;
                div()
                    .flex_col()
                    .gap_2()
                    .child(title_line(view.title.clone()).text_color(colors.foreground))
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(view.detail.clone()),
                    )
                    .children(badge_elements(&view))
            })
            .into_any_element()
    }
}

/// The title text line, shared by the card and its hover popup.
fn title_line(title: SharedString) -> Div {
    div().text_sm().child(title)
}

/// The poster placeholder while an image loads or if it fails.
fn poster_skeleton() -> Skeleton {
    Skeleton::new().w_full().h(px(MEDIA_POSTER_HEIGHT))
}

/// The card's badge wrappers: format, score and progress, when set.
fn badge_elements(view: &MediaCardView) -> Vec<Div> {
    let mut badges = Vec::new();
    if let Some(format) = view.format {
        badges.push(
            div()
                .debug_selector(|| "media-card-format".into())
                .flex_none()
                .child(Tag::secondary().small().child(format.label())),
        );
    }
    if let Some(score) = view.score {
        badges.push(
            div()
                .debug_selector(|| "media-card-score".into())
                .flex_none()
                .child(Tag::secondary().small().child(format!("{score:.1}"))),
        );
    }
    if view.progress > 0 {
        let text = match view.total {
            Some(total) => format!("{}/{total}", view.progress),
            None => format!("{}", view.progress),
        };
        badges.push(
            div()
                .debug_selector(|| "media-card-progress".into())
                .flex_none()
                .child(Tag::secondary().small().child(text)),
        );
    }
    badges
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Context, IntoElement, Render, TestAppContext, Window};

    use super::*;

    fn view() -> MediaCardView {
        MediaCardView {
            poster: Some(Resource::Uri("https://example.com/poster.jpg".into())),
            title: "Frieren: Beyond Journey's End".into(),
            format: Some(MediaFormat::Anime),
            score: Some(8.7),
            progress: 3,
            total: Some(12),
            detail: "A girl who reforms slimes...".into(),
        }
    }

    /// A window root that shows the card.
    struct CardView {
        view: MediaCardView,
        id: &'static str,
    }

    impl Render for CardView {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            MediaCard::new(self.view.clone(), self.id).width(px(160.))
        }
    }

    #[gpui_kit::test]
    fn card_shows_poster_title_and_badges(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: view(),
            id: "media-card-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.debug_bounds("media-card-poster").is_some(),
            "the card shows the poster"
        );
        assert!(
            cx.debug_bounds("media-card-title").is_some(),
            "the card shows the title"
        );
        assert!(
            cx.debug_bounds("media-card-format").is_some(),
            "the card shows the format badge"
        );
        assert!(
            cx.debug_bounds("media-card-score").is_some(),
            "the card shows the score badge"
        );
        assert!(
            cx.debug_bounds("media-card-progress").is_some(),
            "the card shows the progress badge"
        );
    }

    #[gpui_kit::test]
    fn card_without_a_poster_draws_the_skeleton(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: MediaCardView {
                poster: None,
                ..view()
            },
            id: "media-card-skeleton-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.debug_bounds("media-card-skeleton").is_some(),
            "the skeleton draws without a poster"
        );
        assert!(
            cx.debug_bounds("media-card-title").is_some(),
            "the title still draws"
        );
        assert!(
            cx.debug_bounds("media-card-poster").is_none(),
            "no poster area draws without a poster"
        );
    }
}
