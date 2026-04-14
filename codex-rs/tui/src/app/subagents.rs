//! Subagent status-panel orchestration for the TUI app.
//!
//! The app keeps transcript rendering inside `ChatWidget`, but this module owns the mutable
//! registry that turns collab lifecycle events into one live panel plus durable history cells.

use super::*;
use crate::chatwidget::extract_first_bold;
use crate::history_cell::SubagentPanelAgent;
use crate::history_cell::SubagentPanelState;
use crate::history_cell::SubagentStatusCell;
use crate::text_formatting::truncate_text;
use codex_app_server_protocol::CollabAgentState;
use codex_app_server_protocol::CollabAgentStatus;
use codex_app_server_protocol::CollabAgentTool;
use codex_app_server_protocol::CollabAgentToolCallStatus;
use codex_protocol::protocol::AgentMessageDeltaEvent;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::AgentReasoningDeltaEvent;
use codex_protocol::protocol::AgentReasoningRawContentDeltaEvent;
use codex_protocol::protocol::AgentReasoningRawContentEvent;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::CollabAgentSpawnEndEvent;
use codex_protocol::protocol::CollabCloseEndEvent;
use codex_protocol::protocol::CollabWaitingEndEvent;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use std::collections::HashSet;
use std::sync::Mutex as StdMutex;

const SUBAGENT_PROMPT_PREVIEW_BUDGET: usize = 120;
const SUBAGENT_UPDATE_PREVIEW_BUDGET: usize = 160;
const SUBAGENT_PENDING_EVENT_CAPACITY: usize = 12;
pub(super) const SUBAGENT_ANIMATION_TICK: Duration = Duration::from_millis(100);
const SUBAGENT_SHIMMER_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(super) struct SubagentInfo {
    pub(super) ordinal: i32,
    pub(super) name: String,
    pub(super) prompt_preview: String,
    pub(super) is_watchdog: bool,
    pub(super) status: AgentStatus,
    spawned_at: Instant,
    started_at: Option<Instant>,
    latest_summary: String,
    pub(super) latest_preview: String,
    pub(super) latest_update_at: Instant,
    inflight_message: String,
    reasoning_buffer: String,
    notified_terminal: bool,
}

impl SubagentInfo {
    pub(super) fn new(
        ordinal: i32,
        name: String,
        prompt_preview: String,
        is_watchdog: bool,
    ) -> Self {
        let now = Instant::now();
        Self {
            ordinal,
            name,
            prompt_preview: prompt_preview.clone(),
            is_watchdog,
            status: AgentStatus::PendingInit,
            spawned_at: now,
            started_at: None,
            latest_summary: String::new(),
            latest_preview: prompt_preview,
            latest_update_at: now,
            inflight_message: String::new(),
            reasoning_buffer: String::new(),
            notified_terminal: false,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.status, AgentStatus::PendingInit | AgentStatus::Running)
    }

    fn is_watchdog(&self) -> bool {
        self.is_watchdog
    }

    fn is_visible_in_panel(&self) -> bool {
        if self.is_watchdog() {
            matches!(self.status, AgentStatus::PendingInit | AgentStatus::Running)
        } else {
            self.is_running()
        }
    }

    fn is_running_for_panel(&self) -> bool {
        if self.is_watchdog() {
            matches!(self.status, AgentStatus::Running)
        } else {
            self.is_running()
        }
    }

    fn running_started_at(&self) -> Instant {
        self.started_at.unwrap_or(self.spawned_at)
    }

    fn update_preview(&mut self, preview: String) {
        self.latest_preview = preview;
        self.latest_update_at = Instant::now();
    }

    fn update_reasoning_summary(&mut self, delta: &str) {
        self.reasoning_buffer.push_str(delta);
        if let Some(summary) = extract_first_bold(&self.reasoning_buffer) {
            self.latest_summary = truncate_text(summary.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
            self.latest_update_at = Instant::now();
        }
    }

    fn clear_turn_buffers(&mut self) {
        self.inflight_message.clear();
        self.reasoning_buffer.clear();
        self.latest_summary.clear();
    }

    fn should_shimmer(&self, now: Instant) -> bool {
        if self.is_watchdog() && matches!(self.status, AgentStatus::PendingInit) {
            return false;
        }
        self.is_running()
            && now.saturating_duration_since(self.latest_update_at) <= SUBAGENT_SHIMMER_WINDOW
    }
}

#[derive(Debug, Default)]
pub(super) struct SubagentRegistry {
    root_thread_id: Option<ThreadId>,
    pub(super) agents: HashMap<ThreadId, SubagentInfo>,
    pub(super) order: Vec<ThreadId>,
    pending_events: HashMap<ThreadId, Vec<EventMsg>>,
    pending_history: Vec<Box<dyn HistoryCell>>,
    panel_state: Option<Arc<StdMutex<SubagentPanelState>>>,
    panel_cell: Option<Arc<SubagentStatusCell>>,
    animations_enabled: bool,
}

impl SubagentRegistry {
    pub(super) fn new(animations_enabled: bool) -> Self {
        Self {
            animations_enabled,
            ..Self::default()
        }
    }

    pub(super) fn set_root_thread(&mut self, thread_id: ThreadId) {
        self.root_thread_id = Some(thread_id);
    }

    fn is_root_thread(&self, thread_id: ThreadId) -> bool {
        self.root_thread_id == Some(thread_id)
    }

    fn contains(&self, thread_id: ThreadId) -> bool {
        self.agents.contains_key(&thread_id)
    }

    fn on_spawn_end(&mut self, event: &CollabAgentSpawnEndEvent) -> Option<Box<dyn HistoryCell>> {
        let new_thread_id = event.new_thread_id?;
        let is_watchdog = event.new_agent_role.as_deref() == Some("watchdog");
        if is_watchdog {
            self.prune_superseded_watchdogs(new_thread_id);
        }
        if self.contains(new_thread_id) {
            return None;
        }

        let ordinal = i32::try_from(self.order.len())
            .unwrap_or(i32::MAX - 1)
            .saturating_add(1);
        let prompt_preview = prompt_preview(&event.prompt);
        let name = derive_subagent_name(&event.prompt, ordinal);

        let mut info = SubagentInfo::new(ordinal, name.clone(), prompt_preview, is_watchdog);
        info.status = event.status.clone();
        info.latest_preview = info.prompt_preview.clone();
        info.latest_update_at = Instant::now();

        self.order.push(new_thread_id);
        self.agents.insert(new_thread_id, info);

        let early_events = self
            .pending_events
            .remove(&new_thread_id)
            .unwrap_or_default();
        let mut follow_up = Vec::new();
        for msg in early_events {
            follow_up.extend(self.on_agent_event(new_thread_id, &msg));
        }
        for cell in follow_up {
            self.queue_history(cell);
        }

        let prompt_line = prompt_first_line(&event.prompt);
        Some(Box::new(history_cell::new_subagent_spawned_cell(
            &name,
            &prompt_line,
        )))
    }

    fn prune_superseded_watchdogs(&mut self, keep_thread_id: ThreadId) {
        let superseded: HashSet<ThreadId> = self
            .agents
            .iter()
            .filter_map(|(thread_id, info)| {
                (info.is_watchdog && *thread_id != keep_thread_id).then_some(*thread_id)
            })
            .collect();
        if superseded.is_empty() {
            return;
        }

        self.order
            .retain(|thread_id| !superseded.contains(thread_id));
        self.agents
            .retain(|thread_id, _| !superseded.contains(thread_id));
        self.pending_events
            .retain(|thread_id, _| !superseded.contains(thread_id));
    }

    fn on_close_end(&mut self, event: &CollabCloseEndEvent) -> Option<Box<dyn HistoryCell>> {
        let receiver_id = event.receiver_thread_id;
        let info = self.agents.get_mut(&receiver_id)?;
        info.status = event.status.clone();
        info.latest_update_at = Instant::now();

        if is_terminal_status(&info.status) && !info.notified_terminal {
            info.notified_terminal = true;
            let summary = terminal_summary(&info.status);
            return Some(Box::new(history_cell::new_subagent_update_cell(
                &info.name,
                &info.status,
                summary.as_str(),
            )));
        }
        None
    }

    fn on_wait_end(&mut self, event: &CollabWaitingEndEvent) {
        for (thread_id, status) in &event.statuses {
            let Some(info) = self.agents.get_mut(thread_id) else {
                continue;
            };
            info.status = status.clone();
            info.latest_update_at = Instant::now();
        }
    }

    fn on_agent_event(&mut self, thread_id: ThreadId, msg: &EventMsg) -> Vec<Box<dyn HistoryCell>> {
        let Some(info) = self.agents.get_mut(&thread_id) else {
            self.buffer_pending_event(thread_id, msg.clone());
            return Vec::new();
        };

        let mut history = Vec::new();
        match msg {
            EventMsg::TurnStarted(TurnStartedEvent { .. }) => {
                info.clear_turn_buffers();
                info.status = AgentStatus::Running;
                if info.started_at.is_none() {
                    info.started_at = Some(Instant::now());
                }
            }
            EventMsg::AgentReasoningDelta(AgentReasoningDeltaEvent { delta }) => {
                info.update_reasoning_summary(delta);
            }
            EventMsg::AgentReasoningRawContentDelta(AgentReasoningRawContentDeltaEvent {
                delta,
            }) => {
                info.update_reasoning_summary(delta);
            }
            EventMsg::AgentReasoningRawContent(AgentReasoningRawContentEvent { text }) => {
                info.update_reasoning_summary(text);
                info.reasoning_buffer.clear();
            }
            EventMsg::AgentReasoning(_) | EventMsg::AgentReasoningSectionBreak(_) => {
                info.reasoning_buffer.clear();
            }
            EventMsg::AgentMessageDelta(AgentMessageDeltaEvent { delta }) => {
                info.inflight_message.push_str(delta);
                let preview =
                    truncate_text(info.inflight_message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
                info.update_preview(preview);
            }
            EventMsg::AgentMessage(AgentMessageEvent { message, .. }) => {
                info.inflight_message.clear();
                let preview = truncate_text(message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
                info.update_preview(preview);
            }
            EventMsg::TurnComplete(TurnCompleteEvent {
                last_agent_message, ..
            }) => {
                info.inflight_message.clear();
                info.status = AgentStatus::Completed(last_agent_message.clone());
                if !info.notified_terminal {
                    info.notified_terminal = true;
                    let summary = last_agent_message
                        .as_deref()
                        .map(|message| {
                            truncate_text(message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET)
                        })
                        .unwrap_or_else(|| "completed".to_string());
                    history.push(Box::new(history_cell::new_subagent_update_cell(
                        &info.name,
                        &info.status,
                        summary.as_str(),
                    )) as Box<dyn HistoryCell>);
                }
            }
            EventMsg::TurnAborted(TurnAbortedEvent { reason, .. }) => {
                info.inflight_message.clear();
                let reason_text = format!("{reason:?}").to_lowercase();
                info.status = AgentStatus::Errored(reason_text.clone());
                if !info.notified_terminal {
                    info.notified_terminal = true;
                    history.push(Box::new(history_cell::new_subagent_update_cell(
                        &info.name,
                        &info.status,
                        reason_text.as_str(),
                    )) as Box<dyn HistoryCell>);
                }
            }
            EventMsg::Error(ErrorEvent { message, .. }) => {
                info.inflight_message.clear();
                let summary = truncate_text(message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
                info.status = AgentStatus::Errored(summary.clone());
                if !info.notified_terminal {
                    info.notified_terminal = true;
                    history.push(Box::new(history_cell::new_subagent_update_cell(
                        &info.name,
                        &info.status,
                        summary.as_str(),
                    )) as Box<dyn HistoryCell>);
                }
            }
            EventMsg::ShutdownComplete => {
                info.inflight_message.clear();
                info.status = AgentStatus::Shutdown;
                if !info.notified_terminal {
                    info.notified_terminal = true;
                    history.push(Box::new(history_cell::new_subagent_update_cell(
                        &info.name,
                        &info.status,
                        "shutdown",
                    )) as Box<dyn HistoryCell>);
                }
            }
            _ => {}
        }

        if history.is_empty() && matches!(msg, EventMsg::TurnStarted(_)) {
            info.latest_update_at = Instant::now();
        }

        history
    }

    fn buffer_pending_event(&mut self, thread_id: ThreadId, msg: EventMsg) {
        if self.is_root_thread(thread_id) {
            return;
        }
        let entry = self.pending_events.entry(thread_id).or_default();
        entry.push(msg);
        if entry.len() > SUBAGENT_PENDING_EVENT_CAPACITY {
            let excess = entry.len() - SUBAGENT_PENDING_EVENT_CAPACITY;
            entry.drain(0..excess);
        }
    }

    fn queue_history(&mut self, cell: Box<dyn HistoryCell>) {
        self.pending_history.push(cell);
    }

    fn take_pending_history(&mut self) -> Vec<Box<dyn HistoryCell>> {
        std::mem::take(&mut self.pending_history)
    }

    pub(super) fn has_animating_agents(&self) -> bool {
        let now = Instant::now();
        self.agents.values().any(|info| info.should_shimmer(now))
    }

    fn rebuild_panel_state(&mut self) {
        let mut running_infos: Vec<&SubagentInfo> = self
            .agents
            .values()
            .filter(|info| info.is_visible_in_panel())
            .collect();
        running_infos.sort_by_key(|info| info.ordinal);

        if running_infos.is_empty() {
            self.panel_state = None;
            self.panel_cell = None;
            return;
        }

        let started_at = running_infos
            .iter()
            .map(|info| info.running_started_at())
            .min()
            .unwrap_or_else(Instant::now);
        let running_count = i32::try_from(
            running_infos
                .iter()
                .filter(|info| info.is_running_for_panel())
                .count(),
        )
        .unwrap_or(i32::MAX);
        let total_agents = i32::try_from(running_infos.len()).unwrap_or(i32::MAX);
        let running_agents = running_infos
            .into_iter()
            .map(|info| SubagentPanelAgent {
                ordinal: info.ordinal,
                name: info.name.clone(),
                status: info.status.clone(),
                is_watchdog: info.is_watchdog(),
                watchdog_countdown_started_at: info
                    .is_watchdog()
                    .then_some(info.running_started_at()),
                preview: running_preview(info),
                latest_update_at: info.latest_update_at,
            })
            .collect();

        let state = SubagentPanelState {
            started_at,
            total_agents,
            running_count,
            running_agents,
        };

        match &self.panel_state {
            Some(existing) => {
                let mut guard = existing
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *guard = state;
            }
            None => {
                self.panel_state = Some(Arc::new(StdMutex::new(state)));
            }
        }

        if let Some(panel_state) = &self.panel_state {
            self.panel_cell = Some(Arc::new(SubagentStatusCell::new(
                Arc::clone(panel_state),
                self.animations_enabled,
            )));
        }
    }

    pub(super) fn panel_cell(&self) -> Option<Arc<SubagentStatusCell>> {
        self.panel_cell.clone()
    }
}

impl App {
    pub(super) fn subagents_root_active(&self) -> bool {
        self.primary_thread_id.is_some() && self.active_thread_id == self.primary_thread_id
    }

    fn emit_or_queue_subagent_history(&mut self, cell: Box<dyn HistoryCell>) {
        if self.subagents_root_active() {
            self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
        } else {
            self.subagents.queue_history(cell);
        }
    }

    fn flush_subagent_history_if_root_active(&mut self) {
        if !self.subagents_root_active() {
            return;
        }
        let pending = self.subagents.take_pending_history();
        for cell in pending {
            self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
        }
    }

    pub(super) fn update_subagent_animation(&mut self, root_active: bool) {
        let should_run = root_active && self.subagents.has_animating_agents();
        let is_running = self.subagent_anim_running.load(Ordering::Relaxed);
        if should_run && !is_running {
            self.app_event_tx.send(AppEvent::StartSubagentAnimation);
        } else if !should_run && is_running {
            self.app_event_tx.send(AppEvent::StopSubagentAnimation);
        }
    }

    pub(super) fn sync_subagent_panel_state(&mut self) {
        let root_active = self.subagents_root_active();
        self.subagents.rebuild_panel_state();

        if root_active {
            self.flush_subagent_history_if_root_active();
            if let Some(panel) = self.subagents.panel_cell() {
                self.app_event_tx.send(AppEvent::UpdateSubagentPanel(panel));
            } else {
                self.app_event_tx.send(AppEvent::ClearSubagentPanel);
            }
        } else {
            self.app_event_tx.send(AppEvent::ClearSubagentPanel);
        }

        self.update_subagent_animation(root_active);
    }

    #[allow(dead_code)]
    pub(super) fn process_subagent_side_effects(&mut self, thread_id: ThreadId, event: &Event) {
        if self.primary_thread_id == Some(thread_id) {
            self.subagents.set_root_thread(thread_id);
        }

        if self.subagents.is_root_thread(thread_id) {
            match &event.msg {
                EventMsg::CollabAgentSpawnEnd(ev) => {
                    let _ = self.subagents.on_spawn_end(ev);
                }
                EventMsg::CollabWaitingEnd(ev) => {
                    self.subagents.on_wait_end(ev);
                }
                EventMsg::CollabCloseEnd(ev) => {
                    let _ = self.subagents.on_close_end(ev);
                }
                _ => {}
            }
        } else {
            let updates = self.subagents.on_agent_event(thread_id, &event.msg);
            for cell in updates {
                self.emit_or_queue_subagent_history(cell);
            }
        }

        self.sync_subagent_panel_state();
    }

    pub(super) fn process_subagent_notification_side_effects(
        &mut self,
        thread_id: ThreadId,
        notification: &ServerNotification,
    ) {
        if self.primary_thread_id == Some(thread_id) {
            self.subagents.set_root_thread(thread_id);
        }

        if !self.subagents.is_root_thread(thread_id) {
            return;
        }

        let item = match notification {
            ServerNotification::ItemStarted(notification) => &notification.item,
            ServerNotification::ItemCompleted(notification) => &notification.item,
            _ => {
                self.sync_subagent_panel_state();
                return;
            }
        };

        if let ThreadItem::CollabAgentToolCall {
            id,
            tool,
            status,
            sender_thread_id,
            receiver_thread_ids,
            prompt,
            agents_states,
            ..
        } = item
        {
            if matches!(tool, CollabAgentTool::SpawnAgent) {
                let Some(new_thread_id) = receiver_thread_ids
                    .first()
                    .and_then(|thread_id| ThreadId::from_string(thread_id).ok())
                else {
                    self.sync_subagent_panel_state();
                    return;
                };

                let sender_thread_id = ThreadId::from_string(sender_thread_id).unwrap_or(thread_id);
                let entry = self.agent_navigation.get(&new_thread_id);
                let status = agents_states
                    .get(&new_thread_id.to_string())
                    .map(app_server_collab_state_to_agent_status)
                    .unwrap_or(AgentStatus::PendingInit);

                let _ = self.subagents.on_spawn_end(&CollabAgentSpawnEndEvent {
                    call_id: id.clone(),
                    sender_thread_id,
                    new_thread_id: Some(new_thread_id),
                    new_agent_nickname: entry.and_then(|entry| entry.agent_nickname.clone()),
                    new_agent_role: entry.and_then(|entry| entry.agent_role.clone()),
                    prompt: prompt.clone().unwrap_or_default(),
                    model: String::new(),
                    reasoning_effort: ReasoningEffortConfig::Medium,
                    status,
                });
            } else if !matches!(status, CollabAgentToolCallStatus::InProgress) {
                for receiver_thread_id in receiver_thread_ids {
                    let Some(agent_state) = agents_states.get(receiver_thread_id) else {
                        continue;
                    };
                    let Ok(receiver_thread_id) = ThreadId::from_string(receiver_thread_id) else {
                        continue;
                    };
                    let Some(info) = self.subagents.agents.get_mut(&receiver_thread_id) else {
                        continue;
                    };
                    info.status = app_server_collab_state_to_agent_status(agent_state);
                    info.latest_update_at = Instant::now();
                }
            }
        }

        self.sync_subagent_panel_state();
    }
}

fn app_server_collab_state_to_agent_status(state: &CollabAgentState) -> AgentStatus {
    match state.status {
        CollabAgentStatus::PendingInit => AgentStatus::PendingInit,
        CollabAgentStatus::Running => AgentStatus::Running,
        CollabAgentStatus::Completed => AgentStatus::Completed(state.message.clone()),
        CollabAgentStatus::Errored => {
            AgentStatus::Errored(state.message.clone().unwrap_or_default())
        }
        CollabAgentStatus::Interrupted => AgentStatus::Interrupted,
        CollabAgentStatus::Shutdown => AgentStatus::Shutdown,
        CollabAgentStatus::NotFound => AgentStatus::NotFound,
    }
}

fn is_terminal_status(status: &AgentStatus) -> bool {
    matches!(
        status,
        AgentStatus::Completed(_)
            | AgentStatus::Errored(_)
            | AgentStatus::Shutdown
            | AgentStatus::NotFound
    )
}

fn terminal_summary(status: &AgentStatus) -> String {
    match status {
        AgentStatus::Completed(Some(message)) => {
            truncate_text(message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET)
        }
        AgentStatus::Completed(None) => "completed".to_string(),
        AgentStatus::Errored(message) => {
            truncate_text(message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET)
        }
        AgentStatus::Interrupted => "interrupted".to_string(),
        AgentStatus::Shutdown => "shutdown".to_string(),
        AgentStatus::NotFound => "not found".to_string(),
        AgentStatus::PendingInit | AgentStatus::Running => "running".to_string(),
    }
}

fn prompt_first_line(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn prompt_preview(prompt: &str) -> String {
    let first_line = prompt_first_line(prompt);
    truncate_text(first_line.trim(), SUBAGENT_PROMPT_PREVIEW_BUDGET)
}

fn running_preview(info: &SubagentInfo) -> String {
    if !info.latest_summary.trim().is_empty() {
        return truncate_text(info.latest_summary.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
    }
    if !info.inflight_message.trim().is_empty() {
        return truncate_text(info.inflight_message.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
    }
    if !info.latest_preview.trim().is_empty() {
        return truncate_text(info.latest_preview.trim(), SUBAGENT_UPDATE_PREVIEW_BUDGET);
    }
    truncate_text(info.prompt_preview.trim(), SUBAGENT_PROMPT_PREVIEW_BUDGET)
}

fn derive_subagent_name(prompt: &str, ordinal: i32) -> String {
    let first_line = prompt_first_line(prompt);
    let stripped = first_line
        .strip_prefix("Task:")
        .or_else(|| first_line.strip_prefix("task:"))
        .unwrap_or(&first_line)
        .trim();

    let stopwords = [
        "the", "a", "an", "to", "and", "or", "of", "for", "from", "in", "on", "with", "read",
        "file", "task",
    ];

    let tokens: Vec<String> = stripped
        .split_whitespace()
        .map(clean_token)
        .filter(|token| !token.is_empty())
        .filter(|token| !stopwords.contains(&token.as_str()))
        .take(4)
        .collect();

    if tokens.is_empty() {
        return format!("agent-{ordinal}");
    }

    let joined = tokens.join("-");
    truncate_text(&joined, /*max_graphemes*/ 40)
}

fn clean_token(token: &str) -> String {
    token
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect()
}
