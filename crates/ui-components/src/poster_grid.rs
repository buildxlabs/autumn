//! A virtualized poster grid of media cards: `v_virtual_list` rows, columns
//! sized from the container width, wrapped in the bounded image cache.
//! See ARCHITECTURE §5 (grids) and the ticket's acceptance criteria.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui_kit::{
    App, AppContext, Context, ElementId, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, Size, Styled, TestSupportExt, Window, canvas,
    component::{VirtualListScrollHandle, v_virtual_list},
    div, image_cache, px,
};

use crate::{LruImageCache, MEDIA_CARD_HEIGHT, MediaCard, MediaCardView, POSTER_IMAGE_CAPACITY};

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
pub fn poster_columns(container_width: f32) -> u16 {
    let mut columns = 1;
    let mut occupied = POSTER_WIDTH;
    while occupied + POSTER_WIDTH + POSTER_GAP <= container_width {
        columns += 1;
        occupied += POSTER_WIDTH + POSTER_GAP;
    }
    columns
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
    /// The row ranges the last render pass built; only visible rows appear.
    /// Exposed for tests and instrumentation.
    built_rows: Vec<Range<usize>>,
    scroll_handle: VirtualListScrollHandle,
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
        })
    }

    /// The number of columns the grid has with the measured width.
    pub fn columns(&self) -> u16 {
        poster_columns(self.width.get().unwrap_or_default())
    }

    /// The visible row range the last render pass built; only visible rows
    /// appear.
    pub fn built_rows(&self) -> &[Range<usize>] {
        self.built_rows.as_slice()
    }

    /// The grid's scroll handle.
    pub fn scroll_handle(&self) -> VirtualListScrollHandle {
        self.scroll_handle.clone()
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
                            .map(|(i, view)| {
                                MediaCard::new(view.clone(), format!("poster-card-{}", first + i))
                                    .width(px(card_width))
                            })
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

        div()
            .relative()
            .size_full()
            .child(image_cache(self.cache.clone()).child(grid))
            .child(measure)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, IntoElement, Render, Resource, ScrollStrategy, TestAppContext, Window, size,
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
