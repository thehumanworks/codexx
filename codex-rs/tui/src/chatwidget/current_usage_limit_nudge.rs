use codex_app_server_protocol::RateLimitWindow;
use codex_app_server_protocol::UsageLimitNudge;
use codex_app_server_protocol::UsageLimitNudgeAction;
use codex_protocol::account::PlanType;

pub(super) const CURRENT_USAGE_LIMIT_NUDGE_URL: &str = "https://chatgpt.com/codex/settings/usage";
pub(super) const WORKSPACE_OWNER_USAGE_LIMIT_NUDGE_URL: &str = "https://chatgpt.com/admin/billing";
pub(super) const UPGRADE_USAGE_LIMIT_NUDGE_URL: &str = "https://chatgpt.com/explore/pro";

#[derive(Clone, Copy, PartialEq, Eq)]
enum UsageNudgePrefetchWindow {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct UsageNudgePrefetchKey {
    window: UsageNudgePrefetchWindow,
    window_duration_mins: Option<i64>,
    resets_at: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UsageNudgePrefetchThreshold {
    Percent75,
    Percent90,
}

impl UsageNudgePrefetchThreshold {
    fn from_used_percent(used_percent: i32) -> Option<Self> {
        if used_percent >= 90 {
            Some(Self::Percent90)
        } else if used_percent >= 75 {
            Some(Self::Percent75)
        } else {
            None
        }
    }
}

#[derive(Default)]
pub(super) struct CurrentUsageLimitNudgePromptState {
    active: bool,
    pending: Option<UsageLimitNudge>,
    has_shown: bool,
    last_prefetches: Vec<(UsageNudgePrefetchKey, UsageNudgePrefetchThreshold)>,
}

impl CurrentUsageLimitNudgePromptState {
    pub(super) fn update(&mut self, nudge: Option<UsageLimitNudge>) {
        self.active = nudge.is_some();
        self.pending = (!self.has_shown).then_some(nudge).flatten();
    }

    pub(super) fn take_pending(&mut self) -> Option<UsageLimitNudge> {
        let nudge = self.pending.take()?;
        self.has_shown = true;
        Some(nudge)
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn should_prefetch(
        &mut self,
        primary: Option<&RateLimitWindow>,
        secondary: Option<&RateLimitWindow>,
    ) -> bool {
        let mut should_prefetch = false;

        for (window_kind, window) in [
            (UsageNudgePrefetchWindow::Primary, primary),
            (UsageNudgePrefetchWindow::Secondary, secondary),
        ] {
            let Some(window) = window else {
                continue;
            };
            let Some(threshold) =
                UsageNudgePrefetchThreshold::from_used_percent(window.used_percent)
            else {
                // Keep the per-window watermark across transient downward or
                // sparse live updates so stale events cannot re-arm a
                // threshold we already refreshed for in this window.
                continue;
            };

            let key = UsageNudgePrefetchKey {
                window: window_kind,
                window_duration_mins: window.window_duration_mins,
                resets_at: window.resets_at,
            };
            if let Some((_, last_threshold)) = self
                .last_prefetches
                .iter_mut()
                .find(|(last_key, _)| *last_key == key)
            {
                if *last_threshold >= threshold {
                    continue;
                }
                *last_threshold = threshold;
            } else {
                self.last_prefetches.push((key, threshold));
            }
            should_prefetch = true;
        }

        should_prefetch
    }
}

pub(super) fn prompt_subtitle(nudge: &UsageLimitNudge) -> String {
    let action = match nudge.action {
        UsageLimitNudgeAction::AddCredits => "Add credits",
        UsageLimitNudgeAction::Upgrade => "Upgrade",
    };
    format!(
        "You're at {}% of your Codex usage limit. {action} now to keep going?",
        nudge.threshold
    )
}

pub(super) fn prompt_url(nudge: &UsageLimitNudge, plan_type: Option<PlanType>) -> &'static str {
    match nudge.action {
        UsageLimitNudgeAction::Upgrade => UPGRADE_USAGE_LIMIT_NUDGE_URL,
        UsageLimitNudgeAction::AddCredits
            if plan_type.is_some_and(PlanType::is_workspace_account) =>
        {
            WORKSPACE_OWNER_USAGE_LIMIT_NUDGE_URL
        }
        UsageLimitNudgeAction::AddCredits => CURRENT_USAGE_LIMIT_NUDGE_URL,
    }
}
