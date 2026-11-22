//! A bounded image cache for poster grids.
//!
//! GPUI's default image cache keeps every decoded image for the life of
//! the app. [`LruImageCache`] keeps at most a fixed number and releases the
//! least recently used one with `cx.drop_image` when a new image would go
//! over the cap. Wrap a grid's images in `image_cache(entity)` to use it.
//! See ARCHITECTURE §5 (poster images).

use std::{collections::HashMap, sync::Arc};

use gpui_kit::{
    App, AppContext as _, Entity, ImageCache, ImageCacheError, ImageCacheItem, RenderImage,
    Resource, Window, hash,
};

/// How many decoded posters a grid keeps: a few screens of a dense grid.
pub const POSTER_IMAGE_CAPACITY: usize = 256;

/// An image cache that keeps at most `capacity` images, evicting the least
/// recently loaded one.
///
/// Images are keyed by `gpui::hash(resource)`. Every load, including each
/// frame that paints a cached image, marks the image as recently used.
/// The capacity must exceed the images visible at once, or each frame
/// evicts images it is about to paint.
pub struct LruImageCache {
    capacity: usize,
    /// Bumped on every load; an entry's stamp is the tick of its last load.
    tick: u64,
    entries: HashMap<u64, Entry>,
}

struct Entry {
    item: ImageCacheItem,
    last_used: u64,
}

impl std::fmt::Debug for LruImageCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LruImageCache")
            .field("capacity", &self.capacity)
            .field("len", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl LruImageCache {
    /// A cache that keeps at most `capacity` images (at least one).
    ///
    /// When the entity is released, its images are dropped from every
    /// window.
    pub fn new(capacity: usize, cx: &mut App) -> Entity<Self> {
        let capacity = capacity.max(1);
        let cache = cx.new(|_| Self {
            capacity,
            tick: 0,
            entries: HashMap::with_capacity(capacity),
        });
        cx.observe_release(&cache, |cache, cx| {
            for (_, entry) in cache.entries.drain() {
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }

    /// How many images the cache holds, loaded or still loading.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no images.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether the image for `resource` is in the cache.
    #[must_use]
    pub fn contains(&self, resource: &Resource) -> bool {
        self.entries.contains_key(&hash(resource))
    }

    /// Release the least recently used image.
    fn evict_oldest(&mut self, window: &mut Window, cx: &mut App) {
        let oldest = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| *key);
        if let Some(entry) = oldest.and_then(|key| self.entries.remove(&key))
            && let Some(Ok(image)) = entry.item.get()
        {
            cx.drop_image(image, Some(window));
        }
    }
}

impl ImageCache for LruImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        self.tick = self.tick.wrapping_add(1);
        let key = hash(resource);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.tick;
            return entry.item.use_image(window);
        }
        while self.entries.len() >= self.capacity {
            self.evict_oldest(window, cx);
        }
        let item = ImageCacheItem::new(resource, cx);
        let result = item.use_image(window);
        self.entries.insert(
            key,
            Entry {
                item,
                last_used: self.tick,
            },
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use gpui_kit::{
        Context, ImageSource, IntoElement, ParentElement, Render, Styled, TestAppContext,
        WindowHandle, div, image_cache, img, px, test::TestWindowExt as _,
    };

    use super::*;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="6"><rect width="4" height="6"/></svg>"#;

    /// A grid showing some posters through the cache, like the visible
    /// rows of a scrolled poster grid.
    struct Grid {
        cache: Entity<LruImageCache>,
        shown: Vec<Resource>,
    }

    impl Render for Grid {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            div().child(
                image_cache(self.cache.clone()).children(
                    self.shown
                        .iter()
                        .map(|source| img(ImageSource::Resource(source.clone())).size(px(4.))),
                ),
            )
        }
    }

    struct Harness {
        window: WindowHandle<Grid>,
        cache: Entity<LruImageCache>,
        posters: Vec<Resource>,
        _dir: tempfile::TempDir,
    }

    impl Harness {
        /// A window over a cache of `capacity`, with `count` distinct
        /// poster files on disk.
        fn open(cx: &mut TestAppContext, capacity: usize, count: usize) -> Self {
            cx.executor().allow_parking();
            let dir = tempfile::tempdir().unwrap();
            let posters = (0..count).map(|index| poster(dir.path(), index)).collect();
            let cache = cx.update(|cx| LruImageCache::new(capacity, cx));
            let window = cx.add_window({
                let cache = cache.clone();
                move |_, _| Grid {
                    cache,
                    shown: Vec::new(),
                }
            });
            Self {
                window,
                cache,
                posters,
                _dir: dir,
            }
        }

        /// Paint the posters at `indices` and wait for them to decode.
        fn show(&self, cx: &mut TestAppContext, indices: impl IntoIterator<Item = usize>) {
            let shown: Vec<Resource> = indices
                .into_iter()
                .map(|index| self.posters.get(index).unwrap().clone())
                .collect();
            self.window
                .update(cx, |grid, _, cx| {
                    grid.shown = shown;
                    cx.notify();
                })
                .unwrap();
            self.render(cx);
            cx.run_until_parked();
            self.render(cx);
        }

        fn render(&self, cx: &mut TestAppContext) {
            cx.update_window(self.window.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
        }

        fn decoded(&self, cx: &mut TestAppContext) -> usize {
            self.cache.read_with(cx, |cache, _| {
                cache
                    .entries
                    .values()
                    .filter(|entry| matches!(entry.item.get(), Some(Ok(_))))
                    .count()
            })
        }

        fn holds(&self, cx: &mut TestAppContext, index: usize) -> bool {
            let poster = self.posters.get(index).unwrap().clone();
            self.cache.read_with(cx, |cache, _| cache.contains(&poster))
        }
    }

    fn poster(dir: &Path, index: usize) -> Resource {
        let path: PathBuf = dir.join(format!("{index}.svg"));
        std::fs::write(&path, SVG).unwrap();
        Resource::Path(path.into())
    }

    #[gpui_kit::test]
    fn loading_300_posters_keeps_at_most_256_decoded(cx: &mut TestAppContext) {
        let grid = Harness::open(cx, POSTER_IMAGE_CAPACITY, 300);

        // Scroll through the grid ten posters (a row) at a time.
        for row in 0..30 {
            grid.show(cx, row * 10..row * 10 + 10);
        }

        assert_eq!(grid.decoded(cx), POSTER_IMAGE_CAPACITY);
        assert_eq!(grid.cache.read_with(cx, |cache, _| cache.len()), 256);
        assert!(!grid.holds(cx, 0), "the first rows were evicted");
        assert!(grid.holds(cx, 299), "the visible row is kept");
    }

    #[gpui_kit::test]
    fn reloading_a_poster_keeps_it_through_the_next_eviction(cx: &mut TestAppContext) {
        let grid = Harness::open(cx, POSTER_IMAGE_CAPACITY, 257);
        for row in 0..256 / 16 {
            grid.show(cx, row * 16..row * 16 + 16);
        }

        grid.show(cx, [0]);
        grid.show(cx, [256]);

        assert!(grid.holds(cx, 0), "the reloaded poster is recent");
        assert!(!grid.holds(cx, 1), "the least recent poster went instead");
        assert_eq!(grid.decoded(cx), POSTER_IMAGE_CAPACITY);
    }
}
