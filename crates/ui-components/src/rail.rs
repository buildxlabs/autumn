//! The icon rail on the left of the shell: 64px at rest, 260px expanded.
//! See ARCHITECTURE §5 and decision #19.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, Context, DismissEvent, ElementId, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window,
    assets::IconName,
    canvas,
    component::{
        ActiveTheme, Icon, Selectable,
        button::{Button, ButtonVariants},
        menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
        tooltip::Tooltip,
    },
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
/// The vertical gap between rows.
const ITEM_GAP: f32 = 4.;
/// The icon's size; the row's padding keeps it centred while collapsed.
const ICON_SIZE: f32 = 20.;
/// The element id of the "More" button.
const MORE_ID: &str = "rail-more";

/// One rail entry, as the shell describes it.
#[derive(Debug, Clone)]
pub struct RailItem {
    /// Element id, unique within the rail. The rail's callbacks get it back.
    /// The item's badge, if any, has the id `<id>-badge`.
    pub id: SharedString,
    /// Shown next to the icon when expanded, and as the tooltip when not.
    pub label: SharedString,
    /// The item's icon.
    pub icon: IconName,
    /// Whether this item's route is the one on screen.
    pub active: bool,
    /// A count to show as a pill; `None` shows none.
    pub badge: Option<u32>,
    /// Whether the item sits on the rail. An unpinned main item lives in
    /// "More". System items ignore this.
    pub pinned: bool,
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

/// The id passed to `Rail::new`'s selection callback when the account row is
/// clicked.
pub const ACCOUNT_ID: &str = "rail-account";

/// Called with an item's id.
type OnItem = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
/// Called with `true` when the pointer enters the rail, `false` when it leaves.
type OnHover = Box<dyn Fn(&bool, &mut Window, &mut App)>;
/// Called when a control with nothing to pass back is clicked.
type OnAction = Rc<dyn Fn(&mut Window, &mut App)>;
/// Called with `true` when one of the rail's menus opens, `false` when it
/// closes.
type OnMenu = Rc<dyn Fn(&bool, &mut App)>;

/// The shell's callbacks that rows and menus share.
#[derive(Clone)]
#[expect(
    clippy::struct_field_names,
    reason = "each field handles an on_* event, matching Rail's builder methods"
)]
struct Handlers {
    on_select: OnItem,
    on_pin_toggle: Option<OnItem>,
    on_menu: Option<OnMenu>,
}

impl Handlers {
    /// Reports that the menu `cx` builds opened, and that it closed once
    /// dismissed: both menu kinds emit `DismissEvent` when they close.
    fn track_menu(&self, cx: &mut Context<'_, PopupMenu>) {
        let Some(on_menu) = &self.on_menu else {
            return;
        };
        let opened = Rc::clone(on_menu);
        cx.defer(move |cx| opened(&true, cx));
        let closed = Rc::clone(on_menu);
        // A context menu keeps its PopupMenu entity after dismissal, so
        // on_release alone would miss the close.
        cx.subscribe_self(move |_, _: &DismissEvent, cx| closed(&false, cx))
            .detach();
    }
}

/// A vertical strip of route buttons with a system group pinned to the bottom.
///
/// Main items that don't fit the height, and unpinned ones, share a "More"
/// menu; system items never fold. The shell decides the expansion and
/// handles hover, toggling, selection and pinning. The rail only remembers,
/// per element, its measured height and whether to animate its width. In
/// overlay mode the shell places it over the content, after it in paint
/// order; the rail blocks the mouse to what is behind it.
#[derive(IntoElement)]
pub struct Rail {
    items: Vec<RailItem>,
    system: Vec<RailItem>,
    account: SharedString,
    expansion: RailExpansion,
    handlers: Handlers,
    on_hover: Option<OnHover>,
    on_toggle: Option<OnAction>,
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
            handlers: Handlers {
                on_select: Rc::new(on_select),
                on_pin_toggle: None,
                on_menu: None,
            },
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

    /// Offers Unpin on a main item's context menu and Pin in "More"; both run
    /// `on_pin_toggle` with the item's id, and the shell flips its `pinned`.
    #[must_use]
    pub fn on_pin_toggle(
        mut self,
        on_pin_toggle: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handlers.on_pin_toggle = Some(Rc::new(on_pin_toggle));
        self
    }

    /// Runs `on_menu` with `true` when the More or Unpin menu opens and
    /// `false` when it closes. The pointer leaves the rail for an open menu,
    /// so a hover-opened rail should stay open meanwhile.
    #[must_use]
    pub fn on_menu(mut self, on_menu: impl Fn(&bool, &mut App) + 'static) -> Self {
        self.handlers.on_menu = Some(Rc::new(on_menu));
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

/// How many of the `pinned` items show when the main section is `available`
/// pixels tall. The rest fold into "More", which takes a row itself whenever
/// it shows: when some pinned items fold, or when any item is `unpinned`.
fn shown_count(pinned: usize, unpinned: usize, available: f32) -> usize {
    // Counted rather than divided, to stay clear of float-to-int casts.
    let mut rows = 0_usize;
    let mut used = ITEM_SIZE;
    while used <= available && rows <= pinned {
        rows += 1;
        used += ITEM_GAP + ITEM_SIZE;
    }
    if unpinned == 0 && pinned <= rows {
        pinned
    } else {
        rows.saturating_sub(1).min(pinned)
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

/// The colours rows are drawn with, read once from the theme.
#[derive(Debug, Clone, Copy)]
struct Palette {
    active_wash: Hsla,
    active: Hsla,
    muted: Hsla,
    hover: Hsla,
    badge: Hsla,
    badge_text: Hsla,
    avatar: Hsla,
    radius: Pixels,
}

impl Palette {
    fn new(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            active_wash: theme.accent,
            active: theme.primary,
            muted: theme.muted_foreground,
            hover: theme.list_hover,
            badge: theme.danger,
            badge_text: theme.danger_foreground,
            avatar: theme.primary,
            radius: theme.radius,
        }
    }
}

impl RenderOnce for Rail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let expansion = self.expansion;
        let animate = Motion::track(expansion, window, cx);
        let palette = Palette::new(cx);
        let theme = cx.theme();
        let (bg, border) = (theme.sidebar, theme.sidebar_border);

        let toggle = self.on_toggle.map(|on_toggle| {
            let (icon, text) = if expansion.is_expanded() {
                (IconName::PanelLeftClose, "Collapse")
            } else {
                (IconName::PanelLeftOpen, "Expand")
            };
            let item = RailItem {
                id: "rail-toggle".into(),
                label: text.into(),
                icon,
                active: false,
                badge: None,
                pinned: true,
            };
            rail_row(
                item,
                expansion,
                palette,
                move |window, cx| on_toggle(window, cx),
                None,
            )
        });
        let main = main_section(self.items, expansion, palette, &self.handlers, window, cx);
        // System items can't be unpinned.
        let system_handlers = Handlers {
            on_pin_toggle: None,
            ..self.handlers.clone()
        };
        let system = self
            .system
            .into_iter()
            .map(|item| select_row(item, expansion, palette, &system_handlers));

        let (from, to) = if expansion.is_expanded() {
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
            .gap(px(ITEM_GAP))
            .py_2()
            .overflow_hidden()
            .bg(bg)
            .border_r_1()
            .border_color(border)
            .when(expansion == RailExpansion::Overlay, Styled::shadow_lg)
            .when_some(self.on_hover, |rail, on_hover| {
                rail.on_hover(move |hovered, window, cx| on_hover(hovered, window, cx))
            })
            .children(toggle)
            .child(main)
            .children(system)
            .child(account_row(
                self.account,
                expansion,
                palette,
                Rc::clone(&self.handlers.on_select),
            ));

        if animate {
            rail.with_animation(
                ElementId::Name(format!("rail-width-{}", expansion.is_expanded()).into()),
                Animation::new(WIDTH_ANIMATION).with_easing(ease_in_out),
                move |rail, delta| rail.w(px(from + (to - from) * delta)),
            )
            .into_any_element()
        } else {
            rail.into_any_element()
        }
    }
}

/// The main items: as many as fit the measured height, then "More" with the
/// rest and the unpinned ones. The height is measured while painting, and a
/// new height asks for another frame to fold to it.
fn main_section(
    items: Vec<RailItem>,
    expansion: RailExpansion,
    palette: Palette,
    handlers: &Handlers,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let height = window.use_keyed_state("rail-main-height", cx, |_, _| None::<f32>);
    // Until the first measurement, show everything; it is clipped anyway.
    let available = height.read(cx).unwrap_or(f32::MAX);
    let (mut shown, unpinned): (Vec<_>, Vec<_>) = items.into_iter().partition(|item| item.pinned);
    let folded = shown.split_off(shown_count(shown.len(), unpinned.len(), available));
    let more = (!folded.is_empty() || !unpinned.is_empty())
        .then(|| more_button(folded, unpinned, expansion, palette, handlers));

    let measure = canvas(
        move |bounds, window, cx| {
            let measured = Some(bounds.size.height.as_f32());
            let changed = height.update(cx, |height, _| {
                let changed = *height != measured;
                *height = measured;
                changed
            });
            if changed {
                // A refresh is ignored mid-draw, so ask for the next frame.
                window.request_animation_frame();
            }
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();

    div()
        .relative()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(ITEM_GAP))
        .overflow_hidden()
        .child(measure)
        .children(
            shown
                .into_iter()
                .map(|item| select_row(item, expansion, palette, handlers)),
        )
        .children(more)
}

/// The "More" button and its menu: the `folded` and `unpinned` items to go
/// to, then a Pin entry per unpinned item. It is selected when its menu holds
/// the active route.
fn more_button(
    folded: Vec<RailItem>,
    unpinned: Vec<RailItem>,
    expansion: RailExpansion,
    palette: Palette,
    handlers: &Handlers,
) -> impl IntoElement {
    let hidden = || folded.iter().chain(&unpinned);
    let active = hidden().any(|item| item.active);
    // The badges of the items inside, so their counts stay in sight.
    let total = hidden().filter_map(|item| item.badge).sum::<u32>();
    let pill =
        (total > 0).then(|| badge_pill(format!("{MORE_ID}-badge"), total, expansion, palette));
    let handlers = handlers.clone();
    let (inner_pill, corner_pill) = if expansion.is_expanded() {
        (pill, None)
    } else {
        (None, pill)
    };
    // Button centres its content, so the icon and label go in a full-width
    // row that lines up with the other items.
    let content = div()
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .child(
            Icon::new(IconName::Ellipsis)
                .size(px(ICON_SIZE))
                .flex_none(),
        )
        .when(expansion.is_expanded(), |row| {
            row.child(label("More".into()))
        })
        .children(inner_pill);
    let button = Button::new(MORE_ID)
        .ghost()
        .selected(active)
        .w_full()
        .h(px(ITEM_SIZE))
        .px(px((ITEM_SIZE - ICON_SIZE) / 2.))
        .child(content)
        .when(!expansion.is_expanded(), |button| button.tooltip("More"))
        .dropdown_menu(move |menu, _, cx| {
            handlers.track_menu(cx);
            more_menu(menu, &folded, &unpinned, &handlers)
        });
    // Lined up with the rows, so a corner pill sits where theirs do.
    div()
        .relative()
        .flex_none()
        .mx(px(ITEM_MARGIN))
        .child(button)
        .children(corner_pill)
}

fn more_menu(
    mut menu: PopupMenu,
    folded: &[RailItem],
    unpinned: &[RailItem],
    handlers: &Handlers,
) -> PopupMenu {
    for item in folded.iter().chain(unpinned) {
        let on_select = Rc::clone(&handlers.on_select);
        let id = item.id.clone();
        let text = match item.badge {
            Some(count) => format!("{} ({count})", item.label),
            None => item.label.to_string(),
        };
        menu = menu.item(
            PopupMenuItem::new(text)
                .icon(Icon::new(item.icon))
                .checked(item.active)
                .on_click(move |_, window, cx| on_select(&id, window, cx)),
        );
    }
    let Some(on_pin_toggle) = &handlers.on_pin_toggle else {
        return menu;
    };
    if !unpinned.is_empty() {
        menu = menu.separator();
    }
    for item in unpinned {
        let on_pin_toggle = Rc::clone(on_pin_toggle);
        let id = item.id.clone();
        menu = menu.item(
            PopupMenuItem::new(format!("Pin {}", item.label))
                .icon(Icon::new(IconName::Pin))
                .on_click(move |_, window, cx| on_pin_toggle(&id, window, cx)),
        );
    }
    menu
}

/// A row that runs `on_select` when clicked and, with `on_pin_toggle`, offers Unpin
/// on right-click.
fn select_row(
    item: RailItem,
    expansion: RailExpansion,
    palette: Palette,
    handlers: &Handlers,
) -> AnyElement {
    let id = item.id.clone();
    let on_select = Rc::clone(&handlers.on_select);
    let unpin = handlers.on_pin_toggle.as_ref().map(|on_pin_toggle| {
        let on_pin_toggle = Rc::clone(on_pin_toggle);
        let id = id.clone();
        let unpin: OnAction = Rc::new(move |window: &mut Window, cx: &mut App| {
            on_pin_toggle(&id, window, cx);
        });
        (unpin, handlers.clone())
    });
    rail_row(
        item,
        expansion,
        palette,
        move |window, cx| on_select(&id, window, cx),
        unpin,
    )
}

/// A clickable row: icon, then the label when expanded, and a badge pill.
/// Collapsed, the label moves into a tooltip and the pill onto the icon.
/// With `unpin`, right-clicking offers to move the item into "More".
fn rail_row(
    item: RailItem,
    expansion: RailExpansion,
    palette: Palette,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    unpin: Option<(OnAction, Handlers)>,
) -> AnyElement {
    let RailItem {
        id,
        label: text,
        icon,
        active,
        badge,
        pinned: _,
    } = item;
    let (wash, color) = if active {
        (palette.active_wash, palette.active)
    } else {
        (gpui_kit::transparent_black(), palette.muted)
    };
    let pill = badge.map(|count| badge_pill(format!("{id}-badge"), count, expansion, palette));
    let row = div()
        .id(ElementId::Name(id))
        .test_support()
        .relative()
        .h(px(ITEM_SIZE))
        .mx(px(ITEM_MARGIN))
        .px(px((ITEM_SIZE - ICON_SIZE) / 2.))
        .flex()
        .flex_none()
        .items_center()
        .gap_3()
        .rounded(palette.radius)
        .bg(wash)
        .text_color(color)
        .hover(move |style| style.bg(palette.hover))
        .on_click(move |_, window, cx| on_click(window, cx))
        .child(Icon::new(icon).size(px(ICON_SIZE)).flex_none())
        .map(|row| {
            if expansion.is_expanded() {
                row.child(label(text))
            } else {
                row.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
            }
        })
        .children(pill);
    match unpin {
        Some((unpin, handlers)) => row
            .context_menu(move |menu, _, cx| {
                handlers.track_menu(cx);
                let unpin = Rc::clone(&unpin);
                menu.item(
                    PopupMenuItem::new("Unpin")
                        .icon(Icon::new(IconName::PinOff))
                        .on_click(move |_, window, cx| unpin(window, cx)),
                )
            })
            .into_any_element(),
        None => row.into_any_element(),
    }
}

/// A count pill: on the icon's corner when collapsed, trailing when expanded.
fn badge_pill(
    id: String,
    count: u32,
    expansion: RailExpansion,
    palette: Palette,
) -> impl IntoElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .flex_none()
        .min_w(px(16.))
        .h(px(16.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(palette.badge)
        .text_color(palette.badge_text)
        .text_size(px(10.))
        .map(|pill| {
            if expansion.is_expanded() {
                pill
            } else {
                // Over the icon's top-right corner.
                pill.absolute().top(px(4.)).left(px(26.))
            }
        })
        .child(count.to_string())
}

/// The account row: an avatar dot, then `name` when expanded. Clicking it
/// selects [`ACCOUNT_ID`], so the shell can open the Account settings tab.
fn account_row(
    name: SharedString,
    expansion: RailExpansion,
    palette: Palette,
    on_select: OnItem,
) -> impl IntoElement {
    div()
        .id(ACCOUNT_ID)
        .test_support()
        .on_click(move |_, window, cx| on_select(&ACCOUNT_ID.into(), window, cx))
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
                .bg(palette.avatar),
        )
        .when(expansion.is_expanded(), |row| {
            row.child(label(name).text_color(palette.muted))
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

#[cfg(test)]
mod tests {
    use super::shown_count;

    /// Rows are 44px with 4px gaps, so n rows need 48n - 4 px.
    #[test]
    fn overflow_split_folds_the_bottom_items_into_more() {
        // (pinned, unpinned, available height, expected items shown)
        let cases = [
            (4, 0, 188., 4), // exactly 4 rows: everything fits, no "More"
            (4, 0, 400., 4), // room to spare
            (4, 0, 187., 2), // 3 rows fit: "More" takes one, so 2 items show
            (4, 0, 92., 1),  // 2 rows: 1 item + "More"
            (4, 0, 44., 0),  // 1 row: only "More"
            (4, 0, 10., 0),  // no room at all
            (3, 1, 400., 3), // an unpinned item: "More" shows but fits too
            (3, 1, 140., 2), // 3 rows, "More" needed for the unpinned item
            (0, 2, 400., 0), // all unpinned
        ];
        for (pinned, unpinned, available, expected) in cases {
            assert_eq!(
                shown_count(pinned, unpinned, available),
                expected,
                "{pinned} pinned, {unpinned} unpinned in {available}px"
            );
        }
    }
}
