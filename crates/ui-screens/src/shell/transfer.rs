//! The reviewed list-transfer dialog shown over the settings screen.

use domain::{MediaId, TransferAction, TransferPreview};
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    base::{Disableable, Selectable},
    component::{
        WindowExt,
        button::{Button, ButtonVariants},
        label::Label,
    },
    div, px,
};
use ui_state::transfer::{TransferCommand, TransferStore};

/// The dialog's own copy of the preview, so per-entry overrides stay local
/// until the user confirms.
pub(super) struct TransferPreviewView {
    preview: TransferPreview,
    store: Entity<TransferStore>,
}

impl TransferPreviewView {
    pub(super) fn new(preview: TransferPreview, store: Entity<TransferStore>) -> Self {
        Self { preview, store }
    }

    fn set_action(&mut self, media: MediaId, action: TransferAction, cx: &mut Context<'_, Self>) {
        if let Some(line) = self
            .preview
            .entries
            .iter_mut()
            .find(|line| line.media() == media)
        {
            line.action = action;
            cx.notify();
        }
    }

    fn confirm(&self, cx: &mut App) {
        // Route through the store: the shell keeps the returned Task on its
        // own entity, so closing this dialog cannot cancel the transfer.
        let store = self.store.clone();
        let preview = self.preview.clone();
        store.update(cx, |_, cx| {
            TransferStore::request(TransferCommand::Run(preview), cx);
        });
    }
}

impl Render for TransferPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let pending = self.preview.pending().len();
        let conflicts = self.preview.conflicts();
        let mut rows = Vec::new();
        for (index, line) in self.preview.entries.iter().enumerate() {
            let media = line.media();
            let mut row = div()
                .flex()
                .items_center()
                .gap_3()
                .child(Label::new(format!(
                    "Media {}: {}",
                    media.get(),
                    line.source.notes
                )));
            if line.is_conflict() {
                let keep = if let Some(destination) = &line.destination {
                    format!("Keep destination ({})", destination.notes)
                } else {
                    "Keep destination".to_owned()
                };
                let view = cx.entity().downgrade();
                row = row
                    .child(
                        Button::new(format!("transfer-keep-{index}"))
                            .label(keep)
                            .selected(line.action == TransferAction::Keep)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.set_action(media, TransferAction::Keep, cx);
                                });
                            }),
                    )
                    .child({
                        let view = cx.entity().downgrade();
                        Button::new(format!("transfer-replace-{index}"))
                            .label("Use source")
                            .selected(line.action == TransferAction::Replace)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.set_action(media, TransferAction::Replace, cx);
                                });
                            })
                    });
            } else {
                row = row.child(Label::new("Add").text_sm());
            }
            rows.push(row.into_any_element());
        }
        let confirm_view = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(620.))
            .child(Label::new(format!(
                "{pending} entries will be written; {conflicts} conflicts keep the destination by default."
            )))
            .children(rows)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("transfer-cancel")
                            .label("Cancel")
                            .on_click(move |_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("transfer-confirm")
                            .label("Transfer")
                            .primary()
                            .disabled(pending == 0)
                            .on_click(move |_, window, cx| {
                                let _ = confirm_view.update(cx, |view, cx| view.confirm(cx));
                                window.close_dialog(cx);
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use domain::{
        AdvancedScores, FuzzyDate, ListStatus, MediaKind, Score, TransferDirection, TransferEntry,
    };
    use gpui_kit::{AppContext, TestAppContext};

    use super::*;

    fn line(id: i64, notes: &str) -> TransferEntry {
        TransferEntry {
            source: domain::ListEntry {
                media: MediaId::new(id),
                status: ListStatus::Current,
                score: Score::UNSCORED,
                progress: 1,
                progress_volumes: 0,
                repeat: 0,
                started_at: FuzzyDate::UNKNOWN,
                completed_at: FuzzyDate::UNKNOWN,
                notes: notes.to_owned(),
                private: false,
                priority: 0,
                advanced_scores: AdvancedScores::default(),
                custom_lists: Vec::new(),
                hidden_from_status_lists: false,
                created_at: std::time::SystemTime::UNIX_EPOCH,
                updated_at: std::time::SystemTime::UNIX_EPOCH,
            },
            destination: None,
            action: TransferAction::Add,
        }
    }

    #[gpui_kit::test]
    fn a_per_entry_override_travels_with_the_run_command(cx: &mut TestAppContext) {
        let mut conflict = line(1, "source");
        conflict.destination = Some(line(1, "destination").source);
        conflict.action = TransferAction::Keep;
        let preview = TransferPreview {
            direction: TransferDirection::LocalToAniList,
            kind: MediaKind::Anime,
            entries: vec![conflict, line(2, "new")],
            custom_lists: Vec::new(),
        };
        let commands = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&commands);
        let store = cx.update(|cx| cx.new(|_| TransferStore::default()));
        cx.update(|cx| {
            cx.subscribe(&store, move |_, command: &TransferCommand, _| {
                log.borrow_mut().push(command.clone());
            })
            .detach();
        });
        let view = cx.update(|cx| cx.new(|_| TransferPreviewView::new(preview, store)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.set_action(MediaId::new(1), TransferAction::Replace, cx);
            });
        });
        cx.update(|cx| view.update(cx, |view, cx| view.confirm(cx)));

        let commands = commands.borrow();
        let run = commands.iter().find_map(|command| match command {
            TransferCommand::Run(preview) => Some(preview),
            _ => None,
        });
        assert!(run.is_some(), "confirming the preview sends a run command");
        if let Some(run) = run {
            let actions: Vec<_> = run.entries.iter().map(|line| line.action).collect();
            assert_eq!(actions, vec![TransferAction::Replace, TransferAction::Add]);
        }
    }
}
