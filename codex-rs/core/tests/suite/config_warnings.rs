use codex_config::CONFIG_TOML_FILE;
use codex_protocol::config_types::Verbosity;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;

const INVALID_SANDBOX_WARNING: &str =
    "Ignoring invalid config value at sandbox_mode: \"hyperdrive\"";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_enum_config_emits_startup_warning_and_keeps_valid_settings() {
    let server = start_mock_server().await;
    let config_toml = r#"
model = "gpt-4o"
approval_policy = "never"
sandbox_mode = "hyperdrive"
model_verbosity = "high"
"#;
    let mut builder = test_codex().with_pre_build_hook(move |home| {
        std::fs::write(home.join(CONFIG_TOML_FILE), config_toml).expect("seed config.toml");
    });

    let test = builder.build(&server).await.expect("create conversation");

    assert_eq!(
        (
            test.session_configured.model.as_str(),
            test.session_configured.approval_policy,
            test.config.model_verbosity,
        ),
        ("gpt-4o", AskForApproval::Never, Some(Verbosity::High)),
    );

    let warning = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::Warning(WarningEvent { message }) if message == INVALID_SANDBOX_WARNING
        )
    })
    .await;
    let EventMsg::Warning(WarningEvent { message }) = warning else {
        panic!("expected invalid config warning");
    };
    assert_eq!(message, INVALID_SANDBOX_WARNING);
}
