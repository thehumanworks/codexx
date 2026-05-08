use codex_app_server_protocol::HookErrorInfo;
use codex_app_server_protocol::HookMetadata;
use codex_app_server_protocol::HookTrustStatus;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;
use super::selection_popup_common::render_menu_surface;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::key_hint;
use crate::render::renderable::Renderable;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HooksReviewPromptSelection {
    ReviewHooks,
    TrustAllAndContinue,
    ContinueWithoutTrusting,
}

pub(crate) struct HooksReviewPromptView {
    hooks: Vec<HookMetadata>,
    warnings: Vec<String>,
    errors: Vec<HookErrorInfo>,
    highlighted: HooksReviewPromptSelection,
    completion: Option<ViewCompletion>,
    app_event_tx: AppEventSender,
}

impl HooksReviewPromptView {
    pub(crate) fn new(
        hooks: Vec<HookMetadata>,
        warnings: Vec<String>,
        errors: Vec<HookErrorInfo>,
        app_event_tx: AppEventSender,
    ) -> Self {
        Self {
            hooks,
            warnings,
            errors,
            highlighted: HooksReviewPromptSelection::ReviewHooks,
            completion: None,
            app_event_tx,
        }
    }

    fn review_needed_count(&self) -> usize {
        self.hooks
            .iter()
            .filter(|hook| hook_needs_review(hook))
            .count()
    }

    fn move_up(&mut self) {
        self.highlighted = match self.highlighted {
            HooksReviewPromptSelection::ReviewHooks => {
                HooksReviewPromptSelection::ContinueWithoutTrusting
            }
            HooksReviewPromptSelection::TrustAllAndContinue => {
                HooksReviewPromptSelection::ReviewHooks
            }
            HooksReviewPromptSelection::ContinueWithoutTrusting => {
                HooksReviewPromptSelection::TrustAllAndContinue
            }
        };
    }

    fn move_down(&mut self) {
        self.highlighted = match self.highlighted {
            HooksReviewPromptSelection::ReviewHooks => {
                HooksReviewPromptSelection::TrustAllAndContinue
            }
            HooksReviewPromptSelection::TrustAllAndContinue => {
                HooksReviewPromptSelection::ContinueWithoutTrusting
            }
            HooksReviewPromptSelection::ContinueWithoutTrusting => {
                HooksReviewPromptSelection::ReviewHooks
            }
        };
    }

    fn select(&mut self) {
        match self.highlighted {
            HooksReviewPromptSelection::ReviewHooks => {
                self.app_event_tx.send(AppEvent::OpenHooksBrowser {
                    hooks: self.hooks.clone(),
                    warnings: self.warnings.clone(),
                    errors: self.errors.clone(),
                });
                self.completion = Some(ViewCompletion::Accepted);
            }
            HooksReviewPromptSelection::TrustAllAndContinue => {
                self.trust_all_hooks();
                self.completion = Some(ViewCompletion::Accepted);
            }
            HooksReviewPromptSelection::ContinueWithoutTrusting => {
                self.completion = Some(ViewCompletion::Cancelled);
            }
        }
    }

    fn trust_all_hooks(&self) {
        for hook in self.hooks.iter().filter(|hook| hook_needs_review(hook)) {
            self.app_event_tx.send(AppEvent::TrustHook {
                key: hook.key.clone(),
                current_hash: hook.current_hash.clone(),
            });
        }
    }

    #[allow(clippy::disallowed_methods)]
    fn prompt_lines(&self) -> Vec<Line<'static>> {
        let count = self.review_needed_count();
        let count_line = match count {
            1 => "1 hook is new or changed.".to_string(),
            count => format!("{count} hooks are new or changed."),
        };
        let options = [
            ("Review hooks", HooksReviewPromptSelection::ReviewHooks),
            (
                "Trust all and continue",
                HooksReviewPromptSelection::TrustAllAndContinue,
            ),
            (
                "Continue without trusting",
                HooksReviewPromptSelection::ContinueWithoutTrusting,
            ),
        ];

        let mut lines = vec![
            "Hooks need review".bold().into(),
            Line::from(count_line).yellow(),
            "Hooks can run outside the sandbox after you trust them."
                .dim()
                .into(),
            Line::default(),
        ];
        lines.extend(
            options
                .into_iter()
                .enumerate()
                .map(|(idx, (label, selection))| {
                    option_line(idx, label, self.highlighted == selection)
                }),
        );
        lines
    }
}

impl BottomPaneView for HooksReviewPromptView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Up, ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_up(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.move_down(),
            KeyEvent {
                code: KeyCode::Char('1'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.highlighted = HooksReviewPromptSelection::ReviewHooks;
                self.select();
            }
            KeyEvent {
                code: KeyCode::Char('2'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.highlighted = HooksReviewPromptSelection::TrustAllAndContinue;
                self.select();
            }
            KeyEvent {
                code: KeyCode::Char('3'),
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                self.highlighted = HooksReviewPromptSelection::ContinueWithoutTrusting;
                self.select();
            }
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.select(),
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.completion = Some(ViewCompletion::Cancelled);
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.completion.is_some()
    }

    fn completion(&self) -> Option<ViewCompletion> {
        self.completion
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.completion = Some(ViewCompletion::Cancelled);
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }
}

impl Renderable for HooksReviewPromptView {
    fn desired_height(&self, _width: u16) -> u16 {
        (self.prompt_lines().len() + 3)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let content_area = render_menu_surface(content_area, buf);
        Paragraph::new(self.prompt_lines()).render(content_area, buf);
        let footer_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        Line::from(vec![
            "Press ".into(),
            key_hint::plain(KeyCode::Enter).into(),
            " to continue; ".into(),
            key_hint::plain(KeyCode::Esc).into(),
            " to continue without trusting".into(),
        ])
        .dim()
        .render(footer_area, buf);
    }
}

fn hook_needs_review(hook: &HookMetadata) -> bool {
    matches!(
        hook.trust_status,
        HookTrustStatus::Untrusted | HookTrustStatus::Modified
    )
}

fn option_line(index: usize, label: &str, is_selected: bool) -> Line<'static> {
    let line = format!(
        "{} {}. {label}",
        if is_selected { '›' } else { ' ' },
        index + 1
    );
    if is_selected {
        Line::from(line).cyan()
    } else {
        Line::from(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;
    use crate::render::renderable::Renderable;
    use crate::test_support::PathBufExt;
    use crate::test_support::test_path_buf;
    use codex_app_server_protocol::HookEventName;
    use codex_app_server_protocol::HookHandlerType;
    use codex_app_server_protocol::HookSource;
    use crossterm::event::KeyEvent;
    use insta::assert_snapshot;
    use ratatui::buffer::Buffer;
    use tokio::sync::mpsc::unbounded_channel;

    fn hook(key: &str, trust_status: HookTrustStatus) -> HookMetadata {
        HookMetadata {
            key: key.to_string(),
            event_name: HookEventName::PreToolUse,
            handler_type: HookHandlerType::Command,
            is_managed: false,
            matcher: Some("Bash".to_string()),
            command: Some("/tmp/hook.sh".to_string()),
            timeout_sec: 30,
            status_message: None,
            source_path: test_path_buf("/tmp/hooks.json").abs(),
            source: HookSource::User,
            plugin_id: None,
            display_order: 0,
            enabled: false,
            current_hash: format!("sha256:{key}"),
            trust_status,
        }
    }

    fn view() -> HooksReviewPromptView {
        let (tx_raw, _rx) = unbounded_channel::<AppEvent>();
        HooksReviewPromptView::new(
            vec![
                hook("path:new", HookTrustStatus::Untrusted),
                hook("path:changed", HookTrustStatus::Modified),
            ],
            Vec::new(),
            Vec::new(),
            AppEventSender::new(tx_raw),
        )
    }

    fn render_lines(view: &HooksReviewPromptView, width: u16) -> String {
        let height = view.desired_height(width);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..area.height)
            .map(|row| {
                let rendered = (0..area.width)
                    .map(|col| {
                        let symbol = buf[(area.x + col, area.y + row)].symbol();
                        if symbol.is_empty() {
                            " ".to_string()
                        } else {
                            symbol.to_string()
                        }
                    })
                    .collect::<String>();
                format!("{rendered:width$}", width = area.width as usize)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_prompt() {
        assert_snapshot!("hooks_review_prompt", render_lines(&view(), /*width*/ 80));
    }

    #[test]
    fn review_selection_opens_hooks_browser() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let mut view = HooksReviewPromptView::new(
            vec![hook("path:new", HookTrustStatus::Untrusted)],
            Vec::new(),
            Vec::new(),
            AppEventSender::new(tx_raw),
        );

        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        assert!(matches!(
            rx.try_recv().expect("open browser event"),
            AppEvent::OpenHooksBrowser { .. }
        ));
        assert_eq!(view.completion(), Some(ViewCompletion::Accepted));
    }

    #[test]
    fn trust_all_selection_trusts_each_review_needed_hook() {
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let mut view = HooksReviewPromptView::new(
            vec![
                hook("path:new", HookTrustStatus::Untrusted),
                hook("path:changed", HookTrustStatus::Modified),
                hook("path:trusted", HookTrustStatus::Trusted),
            ],
            Vec::new(),
            Vec::new(),
            AppEventSender::new(tx_raw),
        );
        view.handle_key_event(KeyEvent::from(KeyCode::Down));

        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        let trust_events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        assert_eq!(trust_events.len(), 2);
        assert!(
            trust_events
                .into_iter()
                .all(|event| matches!(event, AppEvent::TrustHook { .. }))
        );
        assert_eq!(view.completion(), Some(ViewCompletion::Accepted));
    }
}
