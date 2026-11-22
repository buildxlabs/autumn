//! The 64px icon rail on the left of the shell. See ARCHITECTURE §5.

use std::rc::Rc;

use gpui_kit::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window,
    assets::IconName,
    component::{ActiveTheme, Icon, tooltip::Tooltip},
    div, px,
};

/// The rail's width; route content starts right of it.
const RAIL_WIDTH: f32 = 64.;
/// The side of each square item button.
const ITEM_SIZE: f32 = 44.;

/// One rail entry, as the shell describes it.
#[derive(Debug, Clone)]
pub struct RailItem {
    /// Element id, unique within the rail.
    pub id: SharedString,
    /// Shown as the item's tooltip.
    pub label: SharedString,
    /// The item's icon.
    pub icon: IconName,
    /// Whether this item's route is the one on screen.
    pub active: bool,
}

/// Called with the index of the clicked item.
type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A vertical strip of icon buttons, one per route.
///
/// It holds no state: the shell passes the items and handles selection.
#[derive(IntoElement)]
pub struct Rail {
    items: Vec<RailItem>,
    on_select: OnSelect,
}

impl Rail {
    /// A rail showing `items` top to bottom; `on_select` runs with the
    /// clicked item's index.
    pub fn new(
        items: Vec<RailItem>,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            items,
            on_select: Rc::new(on_select),
        }
    }
}

impl std::fmt::Debug for Rail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rail")
            .field("items", &self.items)
            .finish_non_exhaustive()
    }
}

impl RenderOnce for Rail {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let on_select = self.on_select;
        let hover = theme.list_hover;
        div()
            .flex_none()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_2()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .children(self.items.into_iter().enumerate().map(|(index, item)| {
                let on_select = Rc::clone(&on_select);
                let label = item.label;
                let (wash, color) = if item.active {
                    (theme.accent, theme.primary)
                } else {
                    (gpui_kit::transparent_black(), theme.muted_foreground)
                };
                div()
                    .id(ElementId::Name(item.id))
                    .test_support()
                    .size(px(ITEM_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.radius)
                    .bg(wash)
                    .text_color(color)
                    .hover(move |style| style.bg(hover))
                    .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                    .on_click(move |_, window, cx| on_select(index, window, cx))
                    .child(Icon::new(item.icon).size(px(20.)))
            }))
    }
}
