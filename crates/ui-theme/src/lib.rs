//! Design tokens and theme presets, mapped onto the `gpui-component` theme.
//!
//! See ARCHITECTURE §2 and §5.

use gpui_kit::{
    App, Hsla, Pixels,
    component::{Theme, ThemeMode},
    hsla, px, rgb,
};

/// The window background.
fn background() -> Hsla {
    rgb(0x07_07_07).into()
}

/// Panels that sit on the background: the rail, cards, the search pill.
fn paper() -> Hsla {
    rgb(0x0B_0B_0B).into()
}

/// Surfaces raised above paper: popovers and menus.
fn elevated() -> Hsla {
    rgb(0x0D_0D_0D).into()
}

/// The one-pixel border between surfaces.
fn hairline() -> Hsla {
    hsla(0., 0., 1., 0.10)
}

/// The brand color for active and primary states.
fn accent() -> Hsla {
    rgb(0x61_52_DF).into()
}

/// The corner radius of cards, buttons and inputs.
fn radius() -> Pixels {
    px(8.)
}

/// Body text on any surface.
fn foreground() -> Hsla {
    rgb(0xD1_D1_D1).into()
}

/// A faint white wash for hovered and selected rows.
fn wash() -> Hsla {
    hsla(0., 0., 1., 0.06)
}

/// Secondary text and idle icons.
fn muted_foreground() -> Hsla {
    hsla(0., 0., 1., 0.45)
}

/// Switches to the dark theme and applies Autumn's tokens to it.
///
/// Call it once at boot, after `gpui_kit::init`. Widgets then read the
/// tokens through `cx.theme()`, so a later theme change reaches them.
pub fn init(cx: &mut App) {
    // Changing the mode reloads that mode's colors, so the tokens are applied
    // in a second update (see `Theme::update`).
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.background = background();
        colors.foreground = foreground();
        colors.title_bar = background();
        colors.sidebar = paper();
        colors.sidebar_foreground = foreground();
        colors.popover = elevated();
        colors.popover_foreground = foreground();
        colors.border = hairline();
        colors.title_bar_border = hairline();
        colors.sidebar_border = hairline();
        colors.accent = wash();
        colors.list_hover = wash();
        colors.muted = wash();
        colors.muted_foreground = muted_foreground();
        colors.primary = accent();
        colors.sidebar_primary = accent();
        colors.ring = accent();
        theme.radius = radius();
    });
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, component::Theme, hsla, px, rgb};

    #[gpui_kit::test]
    fn init_applies_the_tokens_to_the_component_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::init(cx);

            // The expected values are the spec's literals (ARCHITECTURE §5),
            // not the token functions, so a wrong token fails here.
            let theme = Theme::global(cx);
            assert!(theme.mode.is_dark(), "the shell is dark");
            assert_eq!(theme.background, rgb(0x07_07_07).into(), "background token");
            assert_eq!(theme.sidebar, rgb(0x0B_0B_0B).into(), "paper token");
            assert_eq!(theme.popover, rgb(0x0D_0D_0D).into(), "elevated token");
            assert_eq!(theme.border, hsla(0., 0., 1., 0.10), "hairline token");
            assert_eq!(
                theme.title_bar,
                rgb(0x07_07_07).into(),
                "titlebar uses background"
            );
            assert_eq!(theme.primary, rgb(0x61_52_DF).into(), "accent token");
            assert_eq!(theme.radius, px(8.), "radius token");
        });
    }
}
