//! Preview and progress of a list transfer, received through the event pump.

use domain::{AppEvent, TransferDirection, TransferPreview, TransferProgress};
use gpui_kit::{Context, EventEmitter};

/// A command from the Account tab or the preview dialog to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferCommand {
    /// Read both platforms and publish a preview for the user to review.
    Preview(TransferDirection),
    /// Write the reviewed preview to its destination.
    Run(TransferPreview),
}

/// A change the shell must react to, emitted by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferEvent {
    /// A fresh preview is ready to show in a dialog.
    Preview(TransferPreview),
    /// The transfer finished or failed; any dialog can be closed.
    Finished,
}

/// What the UI knows about the current list transfer.
#[derive(Debug, Default)]
pub struct TransferStore {
    preview: Option<TransferPreview>,
    progress: Option<TransferProgress>,
    error: Option<String>,
}

impl EventEmitter<TransferCommand> for TransferStore {}
impl EventEmitter<TransferEvent> for TransferStore {}

impl TransferStore {
    /// The pending preview, if one is waiting for the user.
    #[must_use]
    pub fn preview(&self) -> Option<&TransferPreview> {
        self.preview.as_ref()
    }

    /// Progress from the latest run, once it has started.
    #[must_use]
    pub fn progress(&self) -> Option<TransferProgress> {
        self.progress
    }

    /// A user-facing failure from planning or running a transfer.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Forward a command to the engine.
    pub fn request(command: TransferCommand, cx: &mut Context<'_, Self>) {
        cx.emit(command);
    }

    /// Apply an event from the foreground pump and notify observing views.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::TransferPreview(preview) => {
                self.preview = Some(preview.clone());
                self.progress = None;
                self.error = None;
                cx.emit(TransferEvent::Preview(preview));
                cx.notify();
            }
            AppEvent::TransferProgress(step) => {
                self.progress = Some(step);
                cx.notify();
            }
            AppEvent::TransferError(message) => {
                self.error = Some(message);
                self.preview = None;
                cx.emit(TransferEvent::Finished);
                cx.notify();
            }
            AppEvent::TransferFinished => {
                self.preview = None;
                self.progress = None;
                cx.emit(TransferEvent::Finished);
                cx.notify();
            }
            _ => {}
        }
    }
}
