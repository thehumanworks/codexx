//! User-message display models and helpers for the chat widget.
//!
//! App-server turn items and queued TUI submissions describe user input in
//! slightly different shapes. This module keeps the display-only representation
//! and comparison keys together so chat rendering can avoid duplicate user rows
//! while preserving local-only attachment metadata.

use std::path::PathBuf;

use codex_protocol::items::UserMessageItem;
use codex_protocol::user_input::TextElement;
use codex_protocol::user_input::UserInput;

use super::ChatWidget;
use super::append_text_with_rebased_elements;

#[derive(Debug, Clone)]
pub(crate) struct UserMessageEvent {
    pub(super) message: String,
    pub(super) images: Option<Vec<String>>,
    pub(super) local_images: Vec<PathBuf>,
    pub(super) text_elements: Vec<TextElement>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RenderedUserMessageEvent {
    pub(super) message: String,
    pub(super) remote_image_urls: Vec<String>,
    pub(super) local_images: Vec<PathBuf>,
    pub(super) text_elements: Vec<TextElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingSteerCompareKey {
    pub(super) message: String,
    pub(super) image_count: usize,
}

impl ChatWidget {
    pub(super) fn rendered_user_message_event_from_parts(
        message: String,
        text_elements: Vec<TextElement>,
        local_images: Vec<PathBuf>,
        remote_image_urls: Vec<String>,
    ) -> RenderedUserMessageEvent {
        RenderedUserMessageEvent {
            message,
            remote_image_urls,
            local_images,
            text_elements,
        }
    }

    pub(super) fn rendered_user_message_event_from_event(
        event: &UserMessageEvent,
    ) -> RenderedUserMessageEvent {
        Self::rendered_user_message_event_from_parts(
            event.message.clone(),
            event.text_elements.clone(),
            event.local_images.clone(),
            event.images.clone().unwrap_or_default(),
        )
    }

    /// Build the compare key for a submitted pending steer without invoking the
    /// expensive request-serialization path. Pending steers only need to match the
    /// committed `ItemCompleted(UserMessage)` emitted after core drains input, which
    /// preserves flattened text and total image count but not UI-only text ranges or
    /// local image paths.
    pub(super) fn pending_steer_compare_key_from_items(
        items: &[UserInput],
    ) -> PendingSteerCompareKey {
        let mut message = String::new();
        let mut image_count = 0;

        for item in items {
            match item {
                UserInput::Text { text, .. } => message.push_str(text),
                UserInput::Image { .. } | UserInput::LocalImage { .. } => image_count += 1,
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        PendingSteerCompareKey {
            message,
            image_count,
        }
    }

    pub(super) fn pending_steer_compare_key_from_item(
        item: &UserMessageItem,
    ) -> PendingSteerCompareKey {
        Self::pending_steer_compare_key_from_items(&item.content)
    }

    pub(super) fn rendered_user_message_event_from_inputs(
        items: &[UserInput],
    ) -> RenderedUserMessageEvent {
        let mut message = String::new();
        let mut remote_image_urls = Vec::new();
        let mut local_images = Vec::new();
        let mut text_elements = Vec::new();

        for item in items {
            match item {
                UserInput::Text {
                    text,
                    text_elements: current_text_elements,
                } => append_text_with_rebased_elements(
                    &mut message,
                    &mut text_elements,
                    text,
                    current_text_elements.iter().map(|element| {
                        TextElement::new(
                            element.byte_range,
                            element.placeholder(text).map(str::to_string),
                        )
                    }),
                ),
                UserInput::Image { image_url } => remote_image_urls.push(image_url.clone()),
                UserInput::LocalImage { path } => local_images.push(path.clone()),
                UserInput::Skill { .. } | UserInput::Mention { .. } => {}
                _ => {}
            }
        }

        Self::rendered_user_message_event_from_parts(
            message,
            text_elements,
            local_images,
            remote_image_urls,
        )
    }
}
