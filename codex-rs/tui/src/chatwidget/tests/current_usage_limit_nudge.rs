use super::*;
use pretty_assertions::assert_eq;

fn snapshot_with_nudge(threshold: u8, action: UsageLimitNudgeAction) -> RateLimitSnapshot {
    RateLimitSnapshot {
        current_usage_limit_nudge: Some(UsageLimitNudge { threshold, action }),
        ..snapshot(f64::from(threshold))
    }
}

fn next_open_url_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> Option<String> {
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::OpenUrlInBrowser { url } = event {
            return Some(url);
        }
    }
    None
}

fn next_rate_limit_refresh_origin(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> Option<RateLimitRefreshOrigin> {
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::RefreshRateLimits { origin } = event {
            return Some(origin);
        }
    }
    None
}

#[tokio::test]
async fn proactive_usage_prompt_renders_backend_actions() {
    let mut rendered_cases = Vec::new();

    for (threshold, action) in [
        (75, UsageLimitNudgeAction::AddCredits),
        (75, UsageLimitNudgeAction::Upgrade),
        (90, UsageLimitNudgeAction::AddCredits),
        (90, UsageLimitNudgeAction::Upgrade),
    ] {
        let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(threshold, action)));
        chat.maybe_show_pending_rate_limit_prompt();
        rendered_cases.push(render_bottom_popup(&chat, /*width*/ 88));
    }

    assert_chatwidget_snapshot!(
        "proactive_usage_prompt_variants",
        rendered_cases.join("\n---\n")
    );
}

#[tokio::test]
async fn proactive_usage_prompt_shows_only_once_per_session() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 75,
        UsageLimitNudgeAction::AddCredits,
    )));
    assert!(chat.maybe_show_pending_current_usage_limit_nudge_prompt());

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::Upgrade,
    )));
    assert!(!chat.maybe_show_pending_current_usage_limit_nudge_prompt());
}

#[tokio::test]
async fn proactive_usage_prompt_empty_snapshot_does_not_queue_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 75.0)));
    assert!(!chat.maybe_show_pending_current_usage_limit_nudge_prompt());
}

#[tokio::test]
async fn proactive_usage_prompt_prefetches_snapshot_at_each_live_threshold_crossing() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 74.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 75.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 80.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 90.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 95.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);
}

#[tokio::test]
async fn proactive_usage_prompt_prefetches_once_when_first_live_update_is_above_90() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 92.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 95.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);
}

#[tokio::test]
async fn proactive_usage_prompt_prefetches_again_after_primary_window_rolls_over() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut first_window = snapshot(/*percent*/ 75.0);
    first_window.primary.as_mut().expect("primary").resets_at = Some(100);
    let mut second_window = snapshot(/*percent*/ 75.0);
    second_window.primary.as_mut().expect("primary").resets_at = Some(200);

    chat.on_live_rate_limit_snapshot(first_window);
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_live_rate_limit_snapshot(second_window);
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );
}

#[tokio::test]
async fn proactive_usage_prompt_prefetch_dedupe_survives_same_window_dip() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 75.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 74.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 80.0));
    assert_eq!(next_rate_limit_refresh_origin(&mut rx), None);

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 90.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );
}

#[tokio::test]
async fn proactive_usage_prompt_empty_prefetch_result_does_not_queue_prompt() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 75.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 75.0)));
    assert!(!chat.maybe_show_pending_current_usage_limit_nudge_prompt());
}

#[tokio::test]
async fn proactive_usage_prompt_yes_opens_expected_destination() {
    for (plan_type, action, expected_url) in [
        (
            None,
            UsageLimitNudgeAction::Upgrade,
            UPGRADE_USAGE_LIMIT_NUDGE_URL,
        ),
        (
            None,
            UsageLimitNudgeAction::AddCredits,
            CURRENT_USAGE_LIMIT_NUDGE_URL,
        ),
        (
            Some(PlanType::SelfServeBusinessUsageBased),
            UsageLimitNudgeAction::AddCredits,
            WORKSPACE_OWNER_USAGE_LIMIT_NUDGE_URL,
        ),
    ] {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.plan_type = plan_type;

        chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(/*threshold*/ 90, action)));
        chat.maybe_show_pending_rate_limit_prompt();
        chat.handle_key_event(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(next_open_url_event(&mut rx), Some(expected_url.to_string()));
    }
}

#[tokio::test]
async fn proactive_usage_prompt_no_dismisses_without_opening_browser() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::Upgrade,
    )));
    chat.maybe_show_pending_rate_limit_prompt();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

    assert_eq!(next_open_url_event(&mut rx), None);
}

#[tokio::test]
async fn proactive_usage_prompt_waits_for_between_turn_hook() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_live_rate_limit_snapshot(snapshot(/*percent*/ 75.0));
    assert_eq!(
        next_rate_limit_refresh_origin(&mut rx),
        Some(RateLimitRefreshOrigin::UsageNudgePrefetch)
    );
    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 75,
        UsageLimitNudgeAction::AddCredits,
    )));
    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(!popup.contains("Approaching usage limit"), "popup: {popup}");

    chat.maybe_show_pending_rate_limit_prompt();
    assert!(render_bottom_popup(&chat, /*width*/ 88).contains("Approaching usage limit"));
}

#[tokio::test]
async fn proactive_usage_prompt_flag_disabled_skips_prompt_and_keeps_passive_warning() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CurrentUsageLimitNudge, /*enabled*/ false);

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::AddCredits,
    )));
    assert!(!chat.maybe_show_pending_current_usage_limit_nudge_prompt());
    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(!popup.contains("Approaching usage limit"), "popup: {popup}");

    let rendered = drain_insert_history(&mut rx)
        .into_iter()
        .map(|lines| lines_to_single_string(&lines))
        .collect::<String>();
    assert!(
        rendered.contains("less than 10% of your 1h limit left"),
        "rendered: {rendered}"
    );
}

#[tokio::test]
async fn proactive_usage_prompt_flag_disable_clears_already_queued_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::AddCredits,
    )));
    chat.set_feature_enabled(Feature::CurrentUsageLimitNudge, /*enabled*/ false);

    assert!(!chat.maybe_show_pending_current_usage_limit_nudge_prompt());
    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(!popup.contains("Approaching usage limit"), "popup: {popup}");
}

#[tokio::test]
async fn proactive_usage_prompt_suppresses_later_rate_limit_switch_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::AddCredits,
    )));
    assert!(matches!(
        chat.rate_limit_switch_prompt,
        RateLimitSwitchPromptState::Idle
    ));

    chat.maybe_show_pending_rate_limit_prompt();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    chat.maybe_show_pending_rate_limit_prompt();

    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(!popup.contains("Approaching rate limits"), "popup: {popup}");
    assert!(matches!(
        chat.rate_limit_switch_prompt,
        RateLimitSwitchPromptState::Idle
    ));
}

#[tokio::test]
async fn proactive_usage_prompt_replaces_shown_rate_limit_switch_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5")).await;
    chat.has_chatgpt_account = true;

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    chat.maybe_show_pending_rate_limit_prompt();
    assert!(render_bottom_popup(&chat, /*width*/ 88).contains("Approaching rate limits"));

    chat.on_rate_limit_snapshot(Some(snapshot_with_nudge(
        /*threshold*/ 90,
        UsageLimitNudgeAction::AddCredits,
    )));
    chat.maybe_show_pending_rate_limit_prompt();
    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(popup.contains("Approaching usage limit"), "popup: {popup}");
    assert!(!popup.contains("Approaching rate limits"), "popup: {popup}");

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    let popup = render_bottom_popup(&chat, /*width*/ 88);
    assert!(!popup.contains("Approaching rate limits"), "popup: {popup}");
}
