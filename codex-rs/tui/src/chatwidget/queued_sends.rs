//! Confirmation and review flow for queued sends paused by usage limits.

use super::*;
use crate::app_event::PausedQueuedInputKind;

const RESUME_QUEUED_SENDS_VIEW_ID: &str = "resume_queued_sends";
const PAUSED_QUEUED_SENDS_REVIEW_VIEW_ID: &str = "paused_queued_sends_review";
const PAUSED_QUEUED_SEND_ACTIONS_VIEW_ID: &str = "paused_queued_send_actions";

impl ChatWidget {
    pub(super) fn pause_queued_sends_after_limit_error(&mut self) {
        if self.has_queued_follow_up_messages() {
            self.queued_sends_paused_after_usage_limit = true;
            self.refresh_pending_input_preview();
        }
    }

    pub(super) fn should_prompt_to_resume_queued_sends(&self) -> bool {
        self.queued_sends_paused_after_usage_limit
            && self.has_queued_follow_up_messages()
            && !self.is_user_turn_pending_or_running()
            && self.bottom_pane.composer_is_empty()
            && self.bottom_pane.no_modal_or_popup_active()
    }

    pub(super) fn show_resume_queued_sends_prompt(&mut self) {
        self.show_selection_view(SelectionViewParams {
            view_id: Some(RESUME_QUEUED_SENDS_VIEW_ID),
            title: Some("Resume queued sends?".to_string()),
            subtitle: Some(
                "Queued inputs were paused after a usage limit was reached.".to_string(),
            ),
            footer_hint: Some(standard_popup_hint_line()),
            initial_selected_idx: Some(0),
            items: vec![
                SelectionItem {
                    name: "Keep paused".to_string(),
                    description: Some(
                        "Leave queued sends paused until you review them later.".to_string(),
                    ),
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Review queued inputs".to_string(),
                    description: Some("Edit or drop queued inputs before resuming.".to_string()),
                    actions: vec![Box::new(|tx| {
                        tx.send(AppEvent::ReviewPausedQueuedSends);
                    })],
                    dismiss_on_select: false,
                    dismiss_parent_on_child_accept: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Resume all queued inputs".to_string(),
                    description: Some("Continue sending queued inputs.".to_string()),
                    actions: vec![Box::new(|tx| {
                        tx.send(AppEvent::ResumeQueuedSends);
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    pub(crate) fn show_paused_queued_sends_review(&mut self) {
        if !self.has_queued_follow_up_messages() {
            self.refresh_pending_input_preview();
            return;
        }
        self.show_selection_view(self.paused_queued_sends_review_params());
    }

    pub(crate) fn show_paused_queued_send_actions(
        &mut self,
        kind: PausedQueuedInputKind,
        index: usize,
    ) {
        let Some(preview) = self.paused_queued_send_preview(kind, index) else {
            self.reopen_paused_queued_sends_review();
            return;
        };

        self.show_selection_view(SelectionViewParams {
            view_id: Some(PAUSED_QUEUED_SEND_ACTIONS_VIEW_ID),
            title: Some("Queued input".to_string()),
            subtitle: Some(preview),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: "Edit queued input".to_string(),
                    description: Some(
                        "Revise this input, then save it back to the queue.".to_string(),
                    ),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::EditPausedQueuedSend { kind, index });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: "Drop from queue".to_string(),
                    description: Some("Remove this input without sending it.".to_string()),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::DropPausedQueuedSend { kind, index });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }

    pub(crate) fn resume_queued_sends(&mut self) {
        self.queued_sends_paused_after_usage_limit = false;
        self.paused_queued_send_edit = None;
        self.refresh_pending_input_preview();
        self.maybe_send_next_queued_input();
    }

    pub(crate) fn edit_paused_queued_send(&mut self, kind: PausedQueuedInputKind, index: usize) {
        let message = match kind {
            PausedQueuedInputKind::Steering => {
                self.rejected_steers_queue.get(index).map(|message| {
                    user_message_for_restore(
                        message.clone(),
                        self.rejected_steer_history_records
                            .get(index)
                            .unwrap_or(&UserMessageHistoryRecord::UserMessageText),
                    )
                })
            }
            PausedQueuedInputKind::FollowUp => {
                self.queued_user_messages.get(index).map(|message| {
                    user_message_for_restore(
                        message.user_message.clone(),
                        self.queued_user_message_history_records
                            .get(index)
                            .unwrap_or(&UserMessageHistoryRecord::UserMessageText),
                    )
                })
            }
        };

        if let Some(message) = message {
            self.paused_queued_send_edit = Some(PausedQueuedSendEdit { kind, index });
            self.restore_user_message_to_composer(message);
            self.refresh_pending_input_preview();
            self.request_redraw();
        }
    }

    pub(crate) fn save_paused_queued_send_edit(&mut self, user_message: UserMessage) -> bool {
        let Some(edit) = self.paused_queued_send_edit.take() else {
            return false;
        };

        match edit.kind {
            PausedQueuedInputKind::Steering => {
                if let Some(message) = self.rejected_steers_queue.get_mut(edit.index) {
                    *message = user_message;
                } else {
                    self.rejected_steers_queue.push_back(user_message);
                    self.rejected_steer_history_records
                        .push_back(UserMessageHistoryRecord::UserMessageText);
                }
                if let Some(history_record) =
                    self.rejected_steer_history_records.get_mut(edit.index)
                {
                    *history_record = UserMessageHistoryRecord::UserMessageText;
                }
            }
            PausedQueuedInputKind::FollowUp => {
                if let Some(message) = self.queued_user_messages.get_mut(edit.index) {
                    message.user_message = user_message;
                } else {
                    self.queued_user_messages
                        .push_back(QueuedUserMessage::from(user_message));
                    self.queued_user_message_history_records
                        .push_back(UserMessageHistoryRecord::UserMessageText);
                }
                if let Some(history_record) =
                    self.queued_user_message_history_records.get_mut(edit.index)
                {
                    *history_record = UserMessageHistoryRecord::UserMessageText;
                }
            }
        }

        self.refresh_pending_input_preview();
        self.show_paused_queued_sends_review();
        self.request_redraw();
        true
    }

    pub(crate) fn drop_paused_queued_send(&mut self, kind: PausedQueuedInputKind, index: usize) {
        let removed = match kind {
            PausedQueuedInputKind::Steering => {
                let removed = self.rejected_steers_queue.remove(index).is_some();
                if removed {
                    self.rejected_steer_history_records.remove(index);
                }
                removed
            }
            PausedQueuedInputKind::FollowUp => {
                let removed = self.queued_user_messages.remove(index).is_some();
                if removed {
                    self.queued_user_message_history_records.remove(index);
                }
                removed
            }
        };

        if !removed {
            self.reopen_paused_queued_sends_review();
            return;
        }

        self.refresh_pending_input_preview();
        if self.has_queued_follow_up_messages() {
            self.reopen_paused_queued_sends_review();
        } else {
            self.request_redraw();
        }
    }

    pub(crate) fn drop_all_paused_queued_sends(&mut self) {
        self.rejected_steers_queue.clear();
        self.rejected_steer_history_records.clear();
        self.queued_user_messages.clear();
        self.queued_user_message_history_records.clear();
        self.paused_queued_send_edit = None;
        self.refresh_pending_input_preview();
        self.request_redraw();
    }

    fn paused_queued_sends_review_params(&self) -> SelectionViewParams {
        let steering_items =
            self.rejected_steers_queue
                .iter()
                .enumerate()
                .map(|(index, message)| {
                    (
                        PausedQueuedInputKind::Steering,
                        index,
                        user_message_preview_text(
                            message,
                            self.rejected_steer_history_records.get(index),
                        ),
                    )
                });
        let follow_up_items =
            self.queued_user_messages
                .iter()
                .enumerate()
                .map(|(index, message)| {
                    (
                        PausedQueuedInputKind::FollowUp,
                        index,
                        user_message_preview_text(
                            &message.user_message,
                            self.queued_user_message_history_records.get(index),
                        ),
                    )
                });

        let mut items = vec![Self::paused_queued_send_section_header("Queued inputs")];
        items.extend(steering_items.chain(follow_up_items).enumerate().map(
            |(display_index, (kind, index, preview))| {
                self.paused_queued_send_review_item(kind, index, display_index + 1, preview)
            },
        ));
        items.push(Self::paused_queued_send_section_spacer());
        items.push(Self::paused_queued_send_section_header("Queue actions"));
        items.push(SelectionItem {
            name: "Resume all queued inputs".to_string(),
            description: Some("Continue sending remaining queued inputs.".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::ResumeQueuedSends);
            })],
            dismiss_on_select: true,
            ..Default::default()
        });
        items.push(SelectionItem {
            name: "Drop all queued inputs".to_string(),
            description: Some("Clear every queued input without sending them.".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::DropAllPausedQueuedSends);
            })],
            dismiss_on_select: true,
            ..Default::default()
        });

        SelectionViewParams {
            view_id: Some(PAUSED_QUEUED_SENDS_REVIEW_VIEW_ID),
            title: Some("Review queued inputs".to_string()),
            subtitle: Some("Select an input to edit or drop before resuming.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            initial_selected_idx: Some(0),
            show_row_numbers: false,
            items,
            ..Default::default()
        }
    }

    fn paused_queued_send_review_item(
        &self,
        kind: PausedQueuedInputKind,
        index: usize,
        display_index: usize,
        preview: String,
    ) -> SelectionItem {
        SelectionItem {
            name: format!("Input {display_index}"),
            description: Some(preview),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenPausedQueuedSendActions { kind, index });
            })],
            dismiss_on_select: false,
            dismiss_parent_on_child_accept: true,
            ..Default::default()
        }
    }

    fn paused_queued_send_section_header(name: &str) -> SelectionItem {
        SelectionItem {
            name: name.to_string(),
            is_disabled: true,
            ..Default::default()
        }
    }

    fn paused_queued_send_section_spacer() -> SelectionItem {
        SelectionItem {
            is_disabled: true,
            ..Default::default()
        }
    }

    fn paused_queued_send_preview(
        &self,
        kind: PausedQueuedInputKind,
        index: usize,
    ) -> Option<String> {
        match kind {
            PausedQueuedInputKind::Steering => {
                self.rejected_steers_queue.get(index).map(|message| {
                    user_message_preview_text(
                        message,
                        self.rejected_steer_history_records.get(index),
                    )
                })
            }
            PausedQueuedInputKind::FollowUp => {
                self.queued_user_messages.get(index).map(|message| {
                    user_message_preview_text(
                        &message.user_message,
                        self.queued_user_message_history_records.get(index),
                    )
                })
            }
        }
    }

    fn reopen_paused_queued_sends_review(&mut self) {
        if !self.has_queued_follow_up_messages() {
            self.refresh_pending_input_preview();
            return;
        }

        let params = self.paused_queued_sends_review_params();
        let replaced = self.bottom_pane.replace_active_views_with_selection_view(
            &[
                RESUME_QUEUED_SENDS_VIEW_ID,
                PAUSED_QUEUED_SENDS_REVIEW_VIEW_ID,
                PAUSED_QUEUED_SEND_ACTIONS_VIEW_ID,
            ],
            params,
        );
        if !replaced {
            self.show_selection_view(self.paused_queued_sends_review_params());
        }
        self.request_redraw();
    }
}
