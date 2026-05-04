use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;

const CONFIG_TOML: &str = "config.toml";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_config_enum_value_emits_startup_warning_and_uses_default() {
    let server = start_mock_server().await;
    let mut builder = test_codex().with_pre_build_hook(|home| {
        std::fs::write(home.join(CONFIG_TOML), "service_tier = \"ultrafast\"\n")
            .expect("seed config.toml");
    });

    let test = builder.build(&server).await.expect("create conversation");
    let warning = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::Warning(WarningEvent { message })
                if message.contains("service_tier") && message.contains("ultrafast")
        )
    })
    .await;

    assert_eq!(None, test.config.service_tier);
    assert_eq!(
        EventMsg::Warning(WarningEvent {
            message: "Ignoring unrecognized config value `ultrafast` for `service_tier`; using the default for this setting.".to_string(),
        }),
        warning
    );
}
