//! A virtualized poster grid of media cards: `v_virtual_list` rows, columns
//! sized from the container width, wrapped in the bounded image cache.
//! See ARCHITECTURE §5 (grids) and the ticket's acceptance criteria.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui_kit::{
    App, AppContext, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, Render, SharedString, Size, Styled, TestSupportExt, Window, canvas,
    component::{VirtualListScrollHandle, v_virtual_list},
    div, image_cache, px,
};

use crate::{LruImageCache, MEDIA_CARD_HEIGHT, MediaCard, MediaCardView, POSTER_IMAGE_CAPACITY};

/// A card action, called with the index of the card that was clicked.
type GridAction = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A scroll action, called when the grid nears its end.
type GridScrollAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// A card's width at the widest common container; [`poster_columns`] targets
/// it, so cards grow with the window until another column fits.
pub const POSTER_WIDTH: f32 = 160.;
/// The gap between cards in a row.
pub const POSTER_GAP: f32 = 16.;
/// The number of columns a poster grid has at a container width.
///
/// The widest count whose cards plus the gaps between them fit; with one
/// column the card is as wide as the container. A narrow or empty container
/// still gets one column, so the grid never collapses to zero.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the floor of a positive pixel count fits u16; negative or NaN widths saturate to zero and are clamped to one"
)]
pub fn poster_columns(container_width: f32) -> u16 {
    (((container_width + POSTER_GAP) / (POSTER_WIDTH + POSTER_GAP)).floor() as u16).max(1)
}

/// A virtualized grid of media cards: one row of cards per `v_virtual_list`
/// item.
///
/// The container width drives the column count, so resizing the window
/// reflows the grid; rows are built lazily, only the visible ones. Images go
/// through the grid's own bounded image cache.
pub struct PosterGrid {
    items: Vec<MediaCardView>,
    /// The measured container width; `None` until the first frame measures
    /// it, so the grid starts at one column.
    width: Rc<Cell<Option<f32>>>,
    cache: Entity<LruImageCache>,
    /// Test-only seam: the visible row range the last render pass built.
    built_rows: Vec<Range<usize>>,
    scroll_handle: VirtualListScrollHandle,
    on_open: Option<GridAction>,
    on_add_to_list: Option<GridAction>,
    /// Called when the user scrolls within one card height of the end.
    on_near_end: Option<GridScrollAction>,
    add_label: Option<SharedString>,
}

impl std::fmt::Debug for PosterGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PosterGrid")
            .field("items", &self.items)
            .field("width", &self.width.get())
            .finish_non_exhaustive()
    }
}

impl PosterGrid {
    /// A grid over `items`, with its own bounded image cache.
    pub fn new(items: Vec<MediaCardView>, cx: &mut App) -> Entity<Self> {
        let cache = LruImageCache::new(POSTER_IMAGE_CAPACITY, cx);
        cx.new(|_| Self {
            items,
            width: Rc::new(Cell::new(None)),
            cache,
            built_rows: Vec::new(),
            scroll_handle: VirtualListScrollHandle::new(),
            on_open: None,
            on_add_to_list: None,
            on_near_end: None,
            add_label: None,
        })
    }

    /// Replaces the cards without recreating the grid or its image cache.
    /// Callers hold the returned entity, so this notifies the view to redraw.
    pub fn set_items(&mut self, items: Vec<MediaCardView>, cx: &mut Context<'_, Self>) {
        self.items = items;
        cx.notify();
    }

    /// Handles Open on a card's context menu. The callback gets the card's
    /// index in the grid. Notify the view after changing this.
    pub fn on_open(&mut self, on_open: impl Fn(usize, &mut Window, &mut App) + 'static) {
        self.on_open = Some(Rc::new(on_open));
    }

    /// Test-only: the Open callback a card's context menu invokes.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn open_action(&self) -> Option<GridAction> {
        self.on_open.clone()
    }

    /// Handles Add to list on a card's context menu. The callback gets the
    /// card's index in the grid. Notify the view after changing this.
    pub fn on_add_to_list(
        &mut self,
        on_add_to_list: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) {
        self.on_add_to_list = Some(Rc::new(on_add_to_list));
    }

    /// Renames the context-menu action on every card, for screens whose
    /// entries are already on a list.
    pub fn set_add_label(&mut self, label: impl Into<SharedString>) {
        self.add_label = Some(label.into());
    }

    /// Builds one card, forwarding the grid's actions with the item's index.
    fn card(&self, index: usize, view: &MediaCardView, card_width: f32) -> MediaCard {
        let mut card =
            MediaCard::new(view.clone(), format!("poster-card-{index}")).width(px(card_width));
        if let Some(on_open) = &self.on_open {
            let on_open = Rc::clone(on_open);
            card = card.on_open(move |window, cx| on_open(index, window, cx));
        }
        if let Some(label) = &self.add_label {
            card = card.add_to_list_label(label.clone());
        }
        if let Some(on_add_to_list) = &self.on_add_to_list {
            let on_add_to_list = Rc::clone(on_add_to_list);
            card = card.on_add_to_list(move |window, cx| on_add_to_list(index, window, cx));
        }
        card
    }

    /// The number of columns the grid has with the measured width.
    pub fn columns(&self) -> u16 {
        poster_columns(self.width.get().unwrap_or_default())
    }

    /// Test-only seam: the visible row range the last render pass built.
    #[doc(hidden)]
    pub fn built_rows(&self) -> &[Range<usize>] {
        self.built_rows.as_slice()
    }

    /// The grid's scroll handle.
    pub fn scroll_handle(&self) -> VirtualListScrollHandle {
        self.scroll_handle.clone()
    }

    /// The grid's own scroll offset, independent of the shell's route view.
    #[must_use]
    pub fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_handle.offset()
    }

    /// Scrolls the grid to `offset`, for restoring a route's saved position.
    pub fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.scroll_handle.set_offset(offset);
    }

    /// Called when the user scrolls within two card heights of the end, so a
    /// caller can page without a Load more button.
    pub fn on_near_end(&mut self, on_near_end: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_near_end = Some(Rc::new(on_near_end));
    }
}

impl Render for PosterGrid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let width = self.width.get().unwrap_or_default();
        let columns = poster_columns(width);
        let column_count = usize::from(columns);
        let card_width = (width - f32::from(columns - 1) * POSTER_GAP) / f32::from(columns);
        let rows = self.items.len().div_ceil(column_count);
        let sizes = Rc::new(
            (0..rows)
                .map(|_| Size::new(px(width), px(MEDIA_CARD_HEIGHT)))
                .collect::<Vec<_>>(),
        );
        let grid = v_virtual_list(
            cx.entity(),
            ElementId::View(cx.entity().entity_id()),
            sizes,
            move |grid, range, _, _| {
                grid.built_rows.clear();
                grid.built_rows.push(range.clone());
                range
                    .map(|row| {
                        let first = row * column_count;
                        let cards = grid
                            .items
                            .get(first..)
                            .into_iter()
                            .flatten()
                            .take(column_count)
                            .enumerate()
                            .map(|(i, view)| grid.card(first + i, view, card_width))
                            .collect::<Vec<_>>();
                        div()
                            .id(ElementId::Name(format!("poster-grid-row-{row}").into()))
                            .test_support()
                            .flex()
                            .gap(px(POSTER_GAP))
                            .h(px(MEDIA_CARD_HEIGHT))
                            .children(cards)
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.scroll_handle);

        // Measure the container width each frame; a change asks for the next
        // frame so the grid re-lays out with the new column count.
        let width_cell = Rc::clone(&self.width);
        let measure = canvas(
            move |bounds, window, _| {
                let measured = bounds.size.width.as_f32();
                if width_cell.get() != Some(measured) {
                    width_cell.set(Some(measured));
                    window.request_animation_frame();
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let near_end = self.on_near_end.clone();
        let handle = self.scroll_handle.clone();
        div()
            .id("poster-grid")
            .test_support()
            .relative()
            .size_full()
            .px_8()
            .on_scroll(move |event, window, cx| {
                if let Some(near_end) = &near_end {
                    // The listener runs before the scroll is applied, so use
                    // the event's delta to predict the resulting offset.
                    let projected = handle.offset().y + event.delta.pixel_delta(px(20.)).y;
                    if handle.max_offset().y + projected <= px(MEDIA_CARD_HEIGHT * 2.) {
                        near_end(window, cx);
                    }
                }
            })
            .child(image_cache(self.cache.clone()).child(grid))
            .child(measure)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::{
        Context, IntoElement, Render, Resource, ScrollStrategy, TestAppContext, Window, px, size,
        test::TestWindowExt as _,
    };

    use super::*;

    /// Mock cards for a grid: one per index, with a distinct poster URL.
    fn mock_views(count: usize) -> Vec<MediaCardView> {
        (0..count)
            .map(|ix| MediaCardView {
                poster: Some(Resource::Uri(format!("mock://poster/{ix}").into())),
                title: format!("Mock {ix}").into(),
                ..MediaCardView::default()
            })
            .collect()
    }

    /// The window root for the grid tests: the grid inside a wrapper view.
    struct GridView {
        grid: Entity<PosterGrid>,
    }

    impl Render for GridView {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            div().size_full().child(self.grid.clone())
        }
    }

    #[gpui_kit::test]
    fn poster_columns_counts_the_widest_fitting_columns() {
        // (container width, columns): 5 cards plus four 16px gaps need
        // 864px, so that fits and 6 (needing 1040px) does not.
        let cases = [
            (0., 1),
            (50., 1),
            (160., 1),
            (335., 1),
            (336., 2),
            (512., 3),
            (864., 5),
            (1000., 5),
        ];
        for (width, expected) in cases {
            assert_eq!(poster_columns(width), expected, "width {width}");
        }
    }

    #[gpui_kit::test]
    fn grid_builds_only_visible_rows(cx: &mut TestAppContext) {
        const COUNT: usize = 2000;
        cx.update(gpui_kit::init);
        let grid = cx.update(|cx| PosterGrid::new(mock_views(COUNT), cx));
        let root_grid = grid.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| GridView { grid: root_grid });
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        }

        let columns = grid.read_with(cx, |grid, _| grid.columns());
        let total_rows = COUNT.div_ceil(usize::from(columns));

        let built = grid.read_with(cx, |grid, _| grid.built_rows().to_vec());
        assert!(!built.is_empty(), "the grid built rows");
        let last = built.last().expect("rows were built");
        assert_eq!(last.start, 0, "the visible window starts at the top");
        assert!(last.end <= total_rows, "built rows are within the grid");
        assert!(
            last.len() < total_rows,
            "only visible rows are built: {last:?} of {total_rows}"
        );

        grid.read_with(cx, |grid, _| {
            grid.scroll_handle()
                .scroll_to_item(total_rows / 2, ScrollStrategy::Top);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let built = grid.read_with(cx, |grid, _| grid.built_rows().to_vec());
        let last = built.last().expect("rows were built");
        assert!(
            last.end > total_rows / 2,
            "the scrolled region is built: {last:?}"
        );
        assert!(
            last.len() < total_rows,
            "still only visible rows are built: {last:?}"
        );
    }

    #[gpui_kit::test]
    fn grid_passes_the_card_index_to_its_menu_callbacks(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let opened = Rc::new(RefCell::new(Vec::new()));
        let added = Rc::new(RefCell::new(Vec::new()));
        let grid = cx.update(|cx| PosterGrid::new(mock_views(100), cx));
        let (opened_grid, added_grid) = (Rc::clone(&opened), Rc::clone(&added));
        grid.update(cx, |grid, cx| {
            grid.on_open(move |index, _, _| opened_grid.borrow_mut().push(index));
            grid.on_add_to_list(move |index, _, _| added_grid.borrow_mut().push(index));
            cx.notify();
        });

        let views = mock_views(100);
        let seven = views.get(7).expect("seven cards");
        let nine = views.get(9).expect("nine cards");
        let card = grid.read_with(cx, |grid, _| grid.card(7, seven, 160.));
        let other = grid.read_with(cx, |grid, _| grid.card(9, nine, 160.));
        let root_grid = grid.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| GridView { grid: root_grid });
        cx.update(|window, cx| {
            card.fire_open(window, cx);
            other.fire_add_to_list(window, cx);
        });

        assert_eq!(&*opened.borrow(), &[7], "Open gets the card's index");
        assert_eq!(&*added.borrow(), &[9], "Add to list gets the card's index");
    }

    #[gpui_kit::test]
    fn set_items_swaps_the_cards_without_recreating_the_grid(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let grid = cx.update(|cx| PosterGrid::new(mock_views(100), cx));
        let cache = grid.read_with(cx, |grid, _| grid.cache.clone());
        let root_grid = grid.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| GridView { grid: root_grid });
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        }

        grid.update(cx, |grid, cx| grid.set_items(mock_views(3), cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();

        assert_eq!(grid.read_with(cx, |grid, _| grid.items.len()), 3);
        assert!(
            cx.update(|window, _| window.try_find("poster-card-2").is_some()),
            "the last new card is drawn"
        );
        assert!(
            cx.update(|window, _| window.try_find("poster-card-3").is_none()),
            "old cards are gone"
        );
        assert!(
            cache == grid.read_with(cx, |grid, _| grid.cache.clone()),
            "the image cache survives the item swap"
        );
    }

    #[gpui_kit::test]
    fn grid_reflows_its_columns_when_the_window_resizes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let grid = cx.update(|cx| PosterGrid::new(mock_views(100), cx));
        let root_grid = grid.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| GridView { grid: root_grid });
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        }
        let wide = grid.read_with(cx, |grid, _| grid.columns());
        assert!(wide > 1, "the grid starts with more than one column");

        cx.simulate_resize(size(px(800.), px(600.)));
        for _ in 0..4 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        }
        let narrow = grid.read_with(cx, |grid, _| grid.columns());

        assert!(
            narrow < wide,
            "columns reflow when the window narrows: {wide} -> {narrow}"
        );
    }
}
