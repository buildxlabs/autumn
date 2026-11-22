//! The icon rail on the left of the shell: 64px at rest, 260px expanded.
//! See ARCHITECTURE §5 and decision #19.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    Animation, AnimationExt, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, SharedString, StatefulInteractiveElement, Styled, TestSupportExt, Window,
    assets::IconName,
    component::{ActiveTheme, Icon, tooltip::Tooltip},
    div, ease_in_out,
    prelude::FluentBuilder,
    px,
};

/// The rail's width at rest. In overlay mode this is also its layout slot.
pub const RAIL_WIDTH: f32 = 64.;
/// The rail's width when expanded.
pub const RAIL_EXPANDED_WIDTH: f32 = 260.;
/// How long the rail takes to widen or narrow.
const WIDTH_ANIMATION: Duration = Duration::from_millis(200);
/// The height of each item row; collapsed, rows are square.
const ITEM_SIZE: f32 = 44.;
/// The gap between a row and the rail's edges, so a collapsed row is centred.
const ITEM_MARGIN: f32 = (RAIL_WIDTH - ITEM_SIZE) / 2.;
/// The icon's size; the row's padding keeps it centred while collapsed.
const ICON_SIZE: f32 = 20.;

/// One rail entry, as the shell describes it.
#[derive(Debug, Clone)]
pub struct RailItem {
    /// Element id, unique within the rail. [`Rail::new`]'s callback gets it
    /// back when the item is clicked.
    pub id: SharedString,
    /// Shown next to the icon when expanded, and as the tooltip when not.
    pub label: SharedString,
    /// The item's icon.
    pub icon: IconName,
    /// Whether this item's route is the one on screen.
    pub active: bool,
}

/// How much room the rail takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum RailExpansion {
    /// Icons only.
    #[default]
    Collapsed,
    /// Expanded over the content; the layout slot stays collapsed-width.
    Overlay,
    /// Expanded in the layout, pushing the content aside.
    Pushed,
}

impl RailExpansion {
    /// Whether labels show.
    pub fn is_expanded(self) -> bool {
        self != Self::Collapsed
    }
}

/// Called with the clicked item's id.
type OnSelect = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
/// Called with `true` when the pointer enters the rail, `false` when it leaves.
type OnHover = Box<dyn Fn(&bool, &mut Window, &mut App)>;
/// Called when the expand/collapse toggle is clicked.
type OnToggle = Rc<dyn Fn(&mut Window, &mut App)>;

/// A vertical strip of route buttons with a system group pinned to the bottom.
///
/// The shell decides the expansion and handles hover, toggling and
/// selection; the rail only remembers, per element, whether to animate its
/// width. In overlay mode the shell places it over the
/// content, after it in paint order; the rail blocks the mouse to what is
/// behind it.
#[derive(IntoElement)]
pub struct Rail {
    items: Vec<RailItem>,
    system: Vec<RailItem>,
    account: SharedString,
    expansion: RailExpansion,
    on_select: OnSelect,
    on_hover: Option<OnHover>,
    on_toggle: Option<OnToggle>,
}

impl Rail {
    /// A collapsed rail with `items` from the top and `system` at the bottom,
    /// above a placeholder account row labelled `account`. `on_select` runs
    /// with the clicked item's id.
    pub fn new(
        items: Vec<RailItem>,
        system: Vec<RailItem>,
        account: impl Into<SharedString>,
        on_select: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            items,
            system,
            account: account.into(),
            expansion: RailExpansion::Collapsed,
            on_select: Rc::new(on_select),
            on_hover: None,
            on_toggle: None,
        }
    }

    /// Sets how much room the rail takes.
    #[must_use]
    pub fn expansion(mut self, expansion: RailExpansion) -> Self {
        self.expansion = expansion;
        self
    }

    /// Runs `on_hover` as the pointer enters (`true`) and leaves (`false`).
    #[must_use]
    pub fn on_hover(mut self, on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(on_hover));
        self
    }

    /// Shows an expand/collapse button at the top that runs `on_toggle`.
    #[must_use]
    pub fn on_toggle(mut self, on_toggle: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }
}

impl std::fmt::Debug for Rail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rail")
            .field("items", &self.items)
            .field("system", &self.system)
            .field("account", &self.account)
            .field("expansion", &self.expansion)
            .finish_non_exhaustive()
    }
}

/// Remembers the expansion between frames, so the width animates only on a
/// change and not when the rail first appears.
#[derive(Debug, Clone, Copy)]
struct Motion {
    expanded: bool,
    /// Set by the first change and kept, so later frames keep animating.
    animate: bool,
}

impl Motion {
    /// Records `expansion` for this frame; true once it has ever changed
    /// between collapsed and expanded.
    fn track(expansion: RailExpansion, window: &mut Window, cx: &mut App) -> bool {
        let expanded = expansion.is_expanded();
        let motion = window.use_keyed_state("rail-motion", cx, |_, _| Self {
            expanded,
            animate: false,
        });
        motion.update(cx, |motion, _| {
            if motion.expanded != expanded {
                *motion = Self {
                    expanded,
                    animate: true,
                };
            }
            motion.animate
        })
    }
}

impl RenderOnce for Rail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let expanded = self.expansion.is_expanded();
        let animate = Motion::track(self.expansion, window, cx);

        let theme = cx.theme();
        let hover = theme.list_hover;
        let radius = theme.radius;
        let muted = theme.muted_foreground;
        let row = |item: RailItem, on_select: &OnSelect| {
            let on_select = Rc::clone(on_select);
            let id = item.id.clone();
            let (wash, color) = if item.active {
                (theme.accent, theme.primary)
            } else {
                (gpui_kit::transparent_black(), muted)
            };
            rail_row(
                RowLook {
                    id: item.id,
                    icon: item.icon,
                    text: item.label,
                    expanded,
                    wash,
                    color,
                    hover,
                    radius,
                },
                move |window, cx| on_select(&id, window, cx),
            )
        };

        let toggle = self.on_toggle.map(|on_toggle| {
            let (icon, text) = if expanded {
                (IconName::PanelLeftClose, "Collapse")
            } else {
                (IconName::PanelLeftOpen, "Expand")
            };
            rail_row(
                RowLook {
                    id: "rail-toggle".into(),
                    icon,
                    text: text.into(),
                    expanded,
                    wash: gpui_kit::transparent_black(),
                    color: muted,
                    hover,
                    radius,
                },
                move |window, cx| on_toggle(window, cx),
            )
        });
        let account = account_row(self.account, self.expansion, theme.primary, muted);

        let on_select = self.on_select;
        let (from, to) = if expanded {
            (RAIL_WIDTH, RAIL_EXPANDED_WIDTH)
        } else {
            (RAIL_EXPANDED_WIDTH, RAIL_WIDTH)
        };
        let rail = div()
            .id("rail")
            .test_support()
            .occlude()
            .flex_none()
            .h_full()
            .w(px(to))
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .overflow_hidden()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .when(self.expansion == RailExpansion::Overlay, |rail| {
                rail.shadow_lg()
            })
            .when_some(self.on_hover, |rail, on_hover| {
                rail.on_hover(move |hovered, window, cx| on_hover(hovered, window, cx))
            })
            .children(toggle)
            .children(self.items.into_iter().map(|item| row(item, &on_select)))
            .child(div().flex_1())
            .children(self.system.into_iter().map(|item| row(item, &on_select)))
            .child(account);

        if animate {
            rail.with_animation(
                ElementId::Name(format!("rail-width-{expanded}").into()),
                Animation::new(WIDTH_ANIMATION).with_easing(ease_in_out),
                move |rail, delta| rail.w(px(from + (to - from) * delta)),
            )
            .into_any_element()
        } else {
            rail.into_any_element()
        }
    }
}

/// What one rail row shows.
struct RowLook {
    id: SharedString,
    icon: IconName,
    text: SharedString,
    expanded: bool,
    wash: Hsla,
    color: Hsla,
    hover: Hsla,
    radius: Pixels,
}

/// A clickable row: icon, then the label when expanded. Collapsed, the label
/// moves into a tooltip.
fn rail_row(look: RowLook, on_click: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    let RowLook {
        id,
        icon,
        text,
        expanded,
        wash,
        color,
        hover,
        radius,
    } = look;
    div()
        .id(ElementId::Name(id))
        .test_support()
        .h(px(ITEM_SIZE))
        .mx(px(ITEM_MARGIN))
        .px(px((ITEM_SIZE - ICON_SIZE) / 2.))
        .flex()
        .flex_none()
        .items_center()
        .gap_3()
        .rounded(radius)
        .bg(wash)
        .text_color(color)
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(Icon::new(icon).size(px(ICON_SIZE)).flex_none())
        .map(|row| {
            if expanded {
                row.child(label(text))
            } else {
                row.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            }
        })
}

/// The placeholder account row: an avatar dot, then `name` when expanded.
fn account_row(
    name: SharedString,
    expansion: RailExpansion,
    avatar: Hsla,
    muted: Hsla,
) -> impl IntoElement {
    div()
        .h(px(ITEM_SIZE))
        .mx(px(ITEM_MARGIN))
        .px(px((ITEM_SIZE - ICON_SIZE) / 2.))
        .flex()
        .flex_none()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .size(px(ICON_SIZE))
                .rounded_full()
                .bg(avatar),
        )
        .when(expansion.is_expanded(), |row| {
            row.child(label(name).text_color(muted))
        })
}

fn label(text: SharedString) -> gpui_kit::Div {
    div()
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
        .text_sm()
        .child(text)
}
