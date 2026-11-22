//! The command palette: route navigation and slash modes. See ARCHITECTURE §5.
//!
//! Search results arrive later (#63). They plug in where [`entries`] returns
//! nothing for a search mode, and where the query callback notifies the view.

use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme, IndexPath,
        command::{Command, CommandItem, CommandState},
    },
    div,
};
use ui_state::{Route, actions::COMMAND_PALETTE};

use crate::shell::{icon, title};

/// Routes the palette offers with no query, in order.
const ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];

/// What the palette lists, chosen by a slash command at the start of the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No slash command: routes, or the slash commands once `/` is typed.
    Navigate,
    /// `/search`: anime and manga together.
    Search,
    /// `/anime`.
    Anime,
    /// `/manga`.
    Manga,
}

impl Mode {
    /// The modes a slash command selects, in the order they are listed.
    const SLASH: [Self; 3] = [Self::Search, Self::Anime, Self::Manga];

    /// The slash command that selects this mode; empty for [`Self::Navigate`].
    fn command(self) -> &'static str {
        match self {
            Self::Navigate => "",
            Self::Search => "/search",
            Self::Anime => "/anime",
            Self::Manga => "/manga",
        }
    }

    /// The query that opens the palette in this mode, ready for typing.
    fn prefix(self) -> SharedString {
        match self {
            Self::Navigate => SharedString::default(),
            Self::Search | Self::Anime | Self::Manga => format!("{} ", self.command()).into(),
        }
    }

    /// Splits a query into its mode and the text searched in that mode.
    ///
    /// A slash command counts only as a whole first word, so `/animes` stays
    /// a navigation query.
    fn parse(query: &str) -> (Self, &str) {
        let (word, rest) = query.split_once(' ').unwrap_or((query, ""));
        Self::SLASH
            .into_iter()
            .find(|mode| mode.command() == word)
            .map_or((Self::Navigate, query), |mode| (mode, rest.trim_start()))
    }
}

/// One row of the palette.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// Navigates to the route.
    Route(Route),
    /// Switches to the mode.
    Mode(Mode),
}

impl Entry {
    fn item(self) -> CommandItem {
        match self {
            Self::Route(route) => CommandItem::new().label(title(route)).icon(icon(route)),
            Self::Mode(mode) => CommandItem::new()
                .label(mode.command())
                .icon(IconName::Search),
        }
    }
}

/// The rows for `mode`. `_text` is what a search mode will search (#63).
fn entries(mode: Mode, _text: &str) -> Vec<Entry> {
    match mode {
        Mode::Navigate => ROUTES.into_iter().map(Entry::Route).collect(),
        Mode::Search | Mode::Anime | Mode::Manga => Vec::new(),
    }
}

/// The rows for the whole query: slash commands once it starts with `/`.
fn entries_for(query: &str) -> (Mode, Vec<Entry>) {
    let (mode, text) = Mode::parse(query);
    let rows = if mode == Mode::Navigate && query.starts_with('/') {
        Mode::SLASH.into_iter().map(Entry::Mode).collect()
    } else {
        entries(mode, text)
    };
    (mode, rows)
}

/// What the palette asks its host to do.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PaletteEvent {
    /// The user picked a route; the host navigates and closes the palette.
    Navigate(Route),
}

/// The palette's view, kept alive between openings by the shell.
///
/// `Command` has no delegate, so the rows are rebuilt from the query on
/// every render.
pub(crate) struct CommandPalette {
    state: Entity<CommandState>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
        }
    }

    /// Starts over in `mode` and focuses the query field.
    pub(crate) fn open(&self, mode: Mode, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_query(mode.prefix(), window, cx);
            state.focus(window, cx);
        });
    }

    /// The query as typed.
    #[cfg(test)]
    pub(crate) fn query(&self, cx: &App) -> SharedString {
        self.state.read(cx).query(cx)
    }

    /// The mode the query selects.
    #[cfg(test)]
    pub(crate) fn mode(&self, cx: &App) -> Mode {
        Mode::parse(&self.query(cx)).0
    }

    fn confirm(&self, index: IndexPath, window: &mut Window, cx: &mut Context<'_, Self>) {
        let query = self.state.read(cx).query(cx);
        match entries_for(&query).1.get(index.row) {
            Some(&Entry::Route(route)) => cx.emit(PaletteEvent::Navigate(route)),
            Some(&Entry::Mode(mode)) => self.open(mode, window, cx),
            None => {}
        }
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let query = self.state.read(cx).query(cx);
        let (mode, rows) = entries_for(&query);
        let on_query = cx.entity().downgrade();
        let on_confirm = cx.entity().downgrade();
        div().key_context(COMMAND_PALETTE).child(
            Command::new(&self.state)
                .bordered(false)
                .items(rows.into_iter().map(Entry::item))
                // Search modes get their rows already matched (#63).
                .filterable(mode == Mode::Navigate)
                .placeholder("Go to a page, or type / for search modes")
                .empty(move |_, _, cx| empty_state(mode, cx))
                .on_query(move |_, _, cx| {
                    // The rows depend on the query, so render them again.
                    if let Some(palette) = on_query.upgrade() {
                        palette.update(cx, |_, cx| cx.notify());
                    }
                })
                .on_confirm(move |index, window, cx| {
                    if let Some(palette) = on_confirm.upgrade() {
                        palette.update(cx, |palette, cx| palette.confirm(index, window, cx));
                    }
                }),
        )
    }
}

fn empty_state(mode: Mode, cx: &App) -> impl IntoElement + use<> {
    let message = match mode {
        Mode::Navigate => "No page or mode matches",
        Mode::Search => "Search isn't available yet",
        Mode::Anime => "Anime search isn't available yet",
        Mode::Manga => "Manga search isn't available yet",
    };
    div()
        .id("palette-empty")
        .py_6()
        .text_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message)
        .test_support()
}
