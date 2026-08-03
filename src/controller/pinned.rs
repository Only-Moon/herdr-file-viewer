//! One frozen, in-memory preview snapshot and its lifecycle operation.

use super::*;

/// The one optional reference preview retained for this controller session.
///
/// The document is an applied render only; the interaction state starts as a clone of the active
/// preview and subsequently receives only pinned-viewport measurements. Neither side shares
/// mutable state with the active preview.
pub(super) struct PinnedSnapshot {
    document: PreviewDocument,
    interaction: PreviewInteractionState,
}

impl Controller {
    pub(super) fn pinned_interaction(&self) -> Option<&PreviewInteractionState> {
        self.pinned_snapshot.as_ref().map(|pin| &pin.interaction)
    }

    pub(super) fn pinned_interaction_mut(&mut self) -> Option<&mut PreviewInteractionState> {
        self.pinned_snapshot
            .as_mut()
            .map(|pin| &mut pin.interaction)
    }

    pub(super) fn pinned_document(&self) -> Option<&PreviewDocument> {
        self.pinned_snapshot.as_ref().map(|pin| &pin.document)
    }

    /// Create, replace, remove, or reject the one in-memory pinned snapshot.
    ///
    /// This is intentionally a controller seam rather than an input action: T-13 owns key
    /// wiring. It never starts a render because a pin is a clone of the already-applied document.
    pub fn pin_active_preview(&mut self) -> Effects {
        let Some(document) = self.active_document().cloned() else {
            self.action_notice = Some(
                match self.active_display {
                    ActiveDisplay::Loading { .. } => "Cannot pin while preview is rendering",
                    ActiveDisplay::Directory { .. } => "Cannot pin a directory",
                    ActiveDisplay::EmptyTree(_) => "Cannot pin: no file is selected",
                    ActiveDisplay::Document(_) => unreachable!("checked above"),
                }
                .into(),
            );
            return Effects::redraw();
        };

        self.action_notice = None;
        if self
            .pinned_snapshot
            .as_ref()
            .is_some_and(|pin| pin.document.origin().identity() == document.origin().identity())
        {
            self.pinned_snapshot = None;
            if self.focus == Focus::Pinned {
                self.focus = Focus::Content;
            }
        } else {
            self.pinned_snapshot = Some(PinnedSnapshot {
                document,
                interaction: self.active_interaction.clone(),
            });
        }
        Effects::redraw()
    }

    /// Project the frozen snapshot for the shared Presenter path.
    pub(super) fn pinned_projection(&self) -> Option<PreviewProjection> {
        let pin = self.pinned_snapshot.as_ref()?;
        let presentation = pin.document.presentation();
        Some(PreviewProjection {
            content: pin.document.content().clone(),
            notices: pin.document.notices().to_vec(),
            flash: None,
            title: None,
            rendering: false,
            scroll: pin.interaction.vertical_scroll,
            hscroll: pin.interaction.horizontal_scroll,
            rows: rendered_rows(
                &pin.interaction,
                &pin.document.content().lines,
                presentation.wrap(),
                0,
            ),
            wrap: presentation.wrap(),
            pad_left: presentation.pad_left(),
            search: pin.interaction.search.as_ref().map(|search| ContentSearch {
                matches: search.matches.clone(),
                current: search.current,
            }),
            line_select: None,
            selection: None,
            origin: Some(pin.document.origin().clone()),
        })
    }

    /// Record the pinned preview's independently measured viewport without triggering work.
    pub(super) fn set_pinned_viewport(&mut self, viewport: Option<(u16, u16)>) {
        let Some((width, height)) = viewport else {
            return;
        };
        let Some(pin) = self.pinned_snapshot.as_mut() else {
            return;
        };
        pin.interaction.viewport_width = width;
        pin.interaction.viewport_height = height;
        clamp_offsets(
            &mut pin.interaction,
            &pin.document.content().lines,
            pin.document.presentation().wrap(),
            0,
        );
    }
}
