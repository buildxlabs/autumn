//! The hero banner: a `bannerImage` backdrop, title, synopsis excerpt and
//! primary actions (#56). Used by Discover's Trending hero and by Entry's
//! header; it takes a plain view model and no media card.

use gpui_kit::{
    Background, ElementId, Entity, Hsla, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    SharedString, Styled, StyledImage, TestSupportExt, div, image_cache, img, linear_color_stop,
    linear_gradient, px,
};

use crate::LruImageCache;

/// How much of the synopsis a hero shows by default.
pub const SYNOPSIS_MAX_CHARS: usize = 180;

/// The text of a hero banner. `banner` is a `bannerImage` URL, which many
/// media lack; the banner renders a gradient when it is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroBannerData {
    /// The media's title.
    pub title: SharedString,
    /// AniList's HTML description, excerpted for the hero.
    pub synopsis: SharedString,
    /// The `bannerImage` URL, when the media has one.
    pub banner: Option<String>,
}

/// Strip AniList's markup and cut the synopsis to a whole word. Returns an
/// empty string when there is nothing to show.
#[must_use]
pub fn synopsis_excerpt(description: &str, max_chars: usize) -> String {
    let plain = strip_markup(description);
    let mut excerpt = String::new();
    let mut cut = false;
    for word in plain.split_whitespace() {
        let used = excerpt.chars().count();
        let needed = usize::from(!excerpt.is_empty()) + word.chars().count();
        if used + needed > max_chars {
            if excerpt.is_empty() {
                let head = word.chars().take(max_chars).collect::<String>();
                return format!("{head}…");
            }
            cut = true;
            break;
        }
        if !excerpt.is_empty() {
            excerpt.push(' ');
        }
        excerpt.push_str(word);
    }
    if cut {
        excerpt.push('…');
    }
    excerpt
}

/// Tags become spaces so `<br>` cannot join two words. Entities AniList
/// actually emits are decoded, with `&amp;` last so `&amp;lt;` stays literal.
fn strip_markup(input: &str) -> String {
    let mut plain = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => {
                if !in_tag {
                    plain.push(' ');
                }
                in_tag = true;
            }
            '>' => in_tag = false,
            _ if !in_tag => plain.push(ch),
            _ => {}
        }
    }
    plain
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// A hero banner. `id` scopes the element ids so a screen can show more than
/// one, as the mock route does. `actions` is the caller's row of primary
/// buttons, so the widget never needs to know what clicking them does.
/// `accent` and `surface` come from the theme, which a plain element cannot
/// read on its own.
pub fn hero_banner(
    id: &str,
    data: HeroBannerData,
    cache: &Entity<LruImageCache>,
    accent: Hsla,
    surface: Hsla,
    actions: impl IntoElement,
) -> impl IntoElement {
    let HeroBannerData {
        title,
        synopsis,
        banner,
    } = data;
    let image_id = ElementId::Name(format!("{id}-image").into());
    let banner_id = ElementId::Name(format!("{id}-banner").into());
    let backdrop = match banner.as_deref() {
        Some(url) => div().absolute().inset_0().child(
            image_cache(cache.clone()).size_full().child(
                img(url)
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Cover)
                    .id(image_id)
                    .test_support(),
            ),
        ),
        None => div()
            .absolute()
            .inset_0()
            .bg(backdrop_gradient(accent, surface)),
    };
    div()
        .id(banner_id)
        .test_support()
        .relative()
        .h(px(300.))
        .w_full()
        .overflow_hidden()
        .rounded(px(12.))
        .child(backdrop)
        .child(div().absolute().inset_0().bg(scrim_gradient()))
        .child(
            div()
                .relative()
                .flex()
                .flex_col()
                .justify_end()
                .gap_3()
                .h_full()
                .p_8()
                .child(div().text_3xl().child(title))
                .child(
                    div()
                        .max_w(px(640.))
                        .text_sm()
                        .child(synopsis_excerpt(&synopsis, SYNOPSIS_MAX_CHARS)),
                )
                .child(actions),
        )
}

/// The fallback a media without a `bannerImage` gets: an accent wash over
/// the surface colour, so the hero still has depth.
fn backdrop_gradient(accent: Hsla, surface: Hsla) -> Background {
    linear_gradient(
        135.,
        linear_color_stop(accent, 0.),
        linear_color_stop(surface, 1.),
    )
}

/// A bottom-up dark scrim so the title and actions stay legible on any image.
fn scrim_gradient() -> Background {
    linear_gradient(
        180.,
        linear_color_stop(
            Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.1,
            },
            0.,
        ),
        linear_color_stop(
            Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.9,
            },
            1.,
        ),
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext, Context, Render, TestAppContext, Window, test::TestWindowExt as _};

    use super::*;

    struct Harness {
        cache: Entity<LruImageCache>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            hero_banner(
                "test",
                HeroBannerData {
                    title: "A title".into(),
                    synopsis: "A synopsis".into(),
                    banner: Some(
                        "https://s4.anilist.co/file/anilistcdn/media/anime/banner/154587-ivXNJ23SM1xB.jpg"
                            .to_owned(),
                    ),
                },
                &self.cache,
                Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.5,
                    a: 1.,
                },
                Hsla {
                    h: 0.,
                    s: 0.,
                    l: 0.05,
                    a: 1.,
                },
                div(),
            )
        }
    }

    #[gpui_kit::test]
    fn a_banner_image_fills_the_hero_height(cx: &mut TestAppContext) {
        let cache = cx.update(|cx| LruImageCache::new(8, cx));
        let window = cx.add_window(|_, _| Harness { cache });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        let height = cx
            .update_window(window.into(), |_, window, _| {
                window.find("test-image").bounds().size.height
            })
            .unwrap();
        assert_eq!(
            height,
            px(300.),
            "the backdrop must fill the banner, not the image's aspect ratio"
        );
    }

    #[test]
    fn synopsis_excerpt_strips_markup_and_truncates_at_a_word() {
        let html = "<i>Fate</i><br> A &amp; B &nbsp; C D E F G H I J";
        assert_eq!(synopsis_excerpt(html, 11), "Fate A & B…");
    }

    #[test]
    fn synopsis_excerpt_hard_cuts_one_overlong_word() {
        assert_eq!(
            synopsis_excerpt("Supercalifragilistic later", 10),
            "Supercalif…"
        );
    }

    #[test]
    fn synopsis_excerpt_keeps_a_short_synopsis_whole() {
        assert_eq!(
            synopsis_excerpt("<p>Short &amp; sweet</p>", 180),
            "Short & sweet"
        );
    }
}
