use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::LOCAL_ENVIRONMENT_ID;
use codex_exec_server::REMOTE_ENVIRONMENT_ID;
use codex_exec_server::RemoveOptions;
use codex_features::Feature;
use codex_protocol::protocol::TurnEnvironmentSelection;
use core_test_support::PathBufExt;
use core_test_support::PathExt;
use core_test_support::assert_regex_match;
use core_test_support::get_remote_test_env;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::skip_if_windows;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use test_case::test_case;

#[cfg(windows)]
const DEFAULT_SHELL_TIMEOUT_MS: i64 = 7_000;
#[cfg(not(windows))]
const DEFAULT_SHELL_TIMEOUT_MS: i64 = 2_000;

#[cfg(windows)]
const MEDIUM_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(windows))]
const MEDIUM_TIMEOUT: Duration = Duration::from_secs(5);

fn shell_responses_with_timeout(
    call_id: &str,
    command: &str,
    login: Option<bool>,
    timeout_ms: i64,
) -> Vec<String> {
    let args = json!({
        "command": command,
        "timeout_ms": timeout_ms,
        "login": login,
    });

    #[allow(clippy::expect_used)]
    let arguments = serde_json::to_string(&args).expect("serialize shell command arguments");

    vec![
        sse(vec![
            ev_response_created("resp-1"),
            ev_function_call(call_id, "shell_command", &arguments),
            ev_completed("resp-1"),
        ]),
        sse(vec![
            ev_assistant_message("msg-1", "done"),
            ev_completed("resp-2"),
        ]),
    ]
}

fn shell_responses(call_id: &str, command: &str, login: Option<bool>) -> Vec<String> {
    shell_responses_with_timeout(call_id, command, login, DEFAULT_SHELL_TIMEOUT_MS)
}

async fn shell_command_harness_with(
    configure: impl FnOnce(TestCodexBuilder) -> TestCodexBuilder,
) -> Result<TestCodexHarness> {
    let builder = configure(test_codex()).with_config(|config| {
        config.include_apply_patch_tool = true;
    });
    TestCodexHarness::with_builder(builder).await
}

async fn remote_aware_shell_command_test(server: &wiremock::MockServer) -> Result<TestCodex> {
    test_codex()
        .with_model("test-gpt-5-codex")
        .with_config(|config| {
            config.use_experimental_unified_exec_tool = false;
            let _ = config.features.disable(Feature::UnifiedExec);
        })
        .build_remote_aware(server)
        .await
}

async fn mount_shell_responses(
    harness: &TestCodexHarness,
    call_id: &str,
    command: &str,
    login: Option<bool>,
) {
    mount_sse_sequence(harness.server(), shell_responses(call_id, command, login)).await;
}

async fn mount_shell_responses_with_timeout(
    harness: &TestCodexHarness,
    call_id: &str,
    command: &str,
    login: Option<bool>,
    timeout: Duration,
) {
    mount_sse_sequence(
        harness.server(),
        shell_responses_with_timeout(call_id, command, login, timeout.as_millis() as i64),
    )
    .await;
}

async fn shell_command_routing_output(
    test: &TestCodex,
    server: &wiremock::MockServer,
    call_id: &str,
    arguments: Value,
    environments: Option<Vec<TurnEnvironmentSelection>>,
) -> Result<String> {
    let response_mock = mount_sse_sequence(
        server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    call_id,
                    "shell_command",
                    &serde_json::to_string(&arguments)?,
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "done"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn_with_environments("route shell command", environments)
        .await?;

    response_mock
        .function_call_output_text(call_id)
        .with_context(|| format!("missing function_call_output for {call_id}"))
}

async fn shell_command_response_mock(
    test: &TestCodex,
    server: &wiremock::MockServer,
    call_id: &str,
    arguments: Value,
    environments: Option<Vec<TurnEnvironmentSelection>>,
) -> Result<core_test_support::responses::ResponseMock> {
    let response_mock = mount_sse_sequence(
        server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    call_id,
                    "shell_command",
                    &serde_json::to_string(&arguments)?,
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-1", "done"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;

    test.submit_turn_with_environments("route shell command", environments)
        .await?;

    Ok(response_mock)
}

fn assert_shell_command_output(output: &str, expected: &str) -> Result<()> {
    let normalized_output = output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string();

    let expected_pattern = format!(
        r"(?s)^Exit code: 0\nWall time: [0-9]+(?:\.[0-9]+)? seconds\nOutput:\n{expected}\n?$"
    );

    assert_regex_match(&expected_pattern, &normalized_output);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_works() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call";
    mount_shell_responses(
        &harness,
        call_id,
        "echo 'hello, world'",
        /*login*/ None,
    )
    .await;
    harness.submit("run the echo command").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "hello, world")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_with_login() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call-login-true";
    mount_shell_responses(&harness, call_id, "echo 'hello, world'", Some(true)).await;
    harness.submit("run the echo command with login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "hello, world")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_without_login() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call-login-false";
    mount_shell_responses(&harness, call_id, "echo 'hello, world'", Some(false)).await;
    harness.submit("run the echo command without login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "hello, world")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_line_output_with_login() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call-first-extra-login";
    mount_shell_responses(
        &harness,
        call_id,
        "echo 'first line\nsecond line'",
        Some(true),
    )
    .await;
    harness.submit("run the command with login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "first line\nsecond line")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipe_output_with_login() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_windows!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call-second-extra-no-login";
    mount_shell_responses(
        &harness,
        call_id,
        "echo 'hello, world' | cat",
        /*login*/ None,
    )
    .await;
    harness.submit("run the command without login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "hello, world")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipe_output_without_login() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    skip_if_windows!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;

    let call_id = "shell-command-call-third-extra-login-false";
    mount_shell_responses(&harness, call_id, "echo 'hello, world' | cat", Some(false)).await;
    harness.submit("run the command without login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "hello, world")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_times_out_with_timeout_ms() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.4")).await?;
    let call_id = "shell-command-timeout";
    let command = if cfg!(windows) {
        "timeout /t 5"
    } else {
        "sleep 5"
    };
    mount_shell_responses_with_timeout(
        &harness,
        call_id,
        command,
        /*login*/ None,
        Duration::from_millis(200),
    )
    .await;
    harness
        .submit("run a long command with a short timeout")
        .await?;

    let output = harness.function_call_stdout(call_id).await;
    let normalized_output = output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string();
    let expected_pattern = r"(?s)^Exit code: 124\nWall time: [0-9]+(?:\.[0-9]+)? seconds\nOutput:\ncommand timed out after [0-9]+ milliseconds\n?$";
    assert_regex_match(expected_pattern, &normalized_output);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_routes_to_selected_remote_environment() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let Some(_remote_env) = get_remote_test_env() else {
        return Ok(());
    };

    let server = start_mock_server().await;
    let test = remote_aware_shell_command_test(&server).await?;
    let local_cwd = TempDir::new()?;
    fs::write(local_cwd.path().join("marker.txt"), "local-routing")?;
    let local_selection = TurnEnvironmentSelection {
        environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
        cwd: local_cwd.path().abs(),
    };
    let remote_cwd = PathBuf::from(format!(
        "/tmp/codex-shell-remote-routing-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ))
    .abs();
    let remote_marker_name = "marker.txt";
    test.fs()
        .create_directory(
            &remote_cwd,
            CreateDirectoryOptions { recursive: true },
            /*sandbox*/ None,
        )
        .await?;
    test.fs()
        .write_file(
            &remote_cwd.join(remote_marker_name),
            b"remote-routing".to_vec(),
            /*sandbox*/ None,
        )
        .await?;
    let remote_selection = TurnEnvironmentSelection {
        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        cwd: remote_cwd.clone(),
    };
    let output = shell_command_routing_output(
        &test,
        &server,
        "call-shell-multi-env",
        json!({
            "command": format!("cat {remote_marker_name}"),
            "login": false,
            "timeout_ms": 1_000,
            "environment_id": REMOTE_ENVIRONMENT_ID,
        }),
        Some(vec![local_selection, remote_selection]),
    )
    .await?;
    assert!(
        output.contains("remote-routing"),
        "unexpected shell_command output: {output}",
    );
    assert!(
        !output.contains("local-routing"),
        "shell_command should not route to local: {output}",
    );

    test.fs()
        .remove(
            &remote_cwd,
            RemoveOptions {
                recursive: true,
                force: true,
            },
            /*sandbox*/ None,
        )
        .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_resolves_relative_workdir_in_selected_remote_environment() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let Some(_remote_env) = get_remote_test_env() else {
        return Ok(());
    };

    let server = start_mock_server().await;
    let test = remote_aware_shell_command_test(&server).await?;
    let local_cwd = TempDir::new()?;
    let local_nested = local_cwd.path().join("nested");
    fs::create_dir_all(&local_nested)?;
    fs::write(local_nested.join("marker.txt"), "local-routing")?;
    let local_selection = TurnEnvironmentSelection {
        environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
        cwd: local_cwd.path().abs(),
    };
    let remote_cwd = PathBuf::from(format!(
        "/tmp/codex-shell-remote-workdir-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ))
    .abs();
    let remote_nested = remote_cwd.join("nested");
    test.fs()
        .create_directory(
            &remote_nested,
            CreateDirectoryOptions { recursive: true },
            /*sandbox*/ None,
        )
        .await?;
    test.fs()
        .write_file(
            &remote_nested.join("marker.txt"),
            b"remote-routing".to_vec(),
            /*sandbox*/ None,
        )
        .await?;
    let remote_selection = TurnEnvironmentSelection {
        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        cwd: remote_cwd.clone(),
    };

    let output = shell_command_routing_output(
        &test,
        &server,
        "call-shell-remote-workdir",
        json!({
            "command": "cat marker.txt",
            "workdir": "nested",
            "login": false,
            "timeout_ms": 1_000,
            "environment_id": REMOTE_ENVIRONMENT_ID,
        }),
        Some(vec![local_selection, remote_selection]),
    )
    .await?;
    assert!(
        output.contains("remote-routing"),
        "unexpected shell_command output: {output}",
    );
    assert!(
        !output.contains("local-routing"),
        "shell_command should resolve workdir in the selected remote environment: {output}",
    );

    test.fs()
        .remove(
            &remote_cwd,
            RemoveOptions {
                recursive: true,
                force: true,
            },
            /*sandbox*/ None,
        )
        .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_command_timeout_in_selected_remote_environment() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let Some(_remote_env) = get_remote_test_env() else {
        return Ok(());
    };

    let server = start_mock_server().await;
    let test = remote_aware_shell_command_test(&server).await?;
    let local_cwd = TempDir::new()?;
    let local_selection = TurnEnvironmentSelection {
        environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
        cwd: local_cwd.path().abs(),
    };
    let remote_cwd = PathBuf::from(format!(
        "/tmp/codex-shell-remote-timeout-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    ))
    .abs();
    test.fs()
        .create_directory(
            &remote_cwd,
            CreateDirectoryOptions { recursive: true },
            /*sandbox*/ None,
        )
        .await?;
    let remote_selection = TurnEnvironmentSelection {
        environment_id: REMOTE_ENVIRONMENT_ID.to_string(),
        cwd: remote_cwd.clone(),
    };

    let response_mock = shell_command_response_mock(
        &test,
        &server,
        "call-shell-remote-timeout",
        json!({
            "command": "sleep 1",
            "login": false,
            "timeout_ms": 50,
            "environment_id": REMOTE_ENVIRONMENT_ID,
        }),
        Some(vec![local_selection, remote_selection]),
    )
    .await?;

    let output_str = response_mock
        .function_call_output_text("call-shell-remote-timeout")
        .expect("timeout output string");

    if let Ok(output_json) = serde_json::from_str::<Value>(&output_str) {
        assert_eq!(
            output_json["metadata"]["exit_code"].as_i64(),
            Some(124),
            "expected timeout exit code 124",
        );
        let stdout = output_json["output"].as_str().unwrap_or_default();
        assert!(
            stdout.contains("command timed out"),
            "timeout output missing `command timed out`: {stdout}"
        );
    } else {
        let lower = output_str.to_lowercase();
        assert!(
            lower.contains("timed out") || lower.contains("signal"),
            "unexpected timeout output: {output_str}",
        );
    }

    test.fs()
        .remove(
            &remote_cwd,
            RemoveOptions {
                recursive: true,
                force: true,
            },
            /*sandbox*/ None,
        )
        .await?;

    Ok(())
}

/// This test verifies that a shell, particularly PowerShell, can correctly
/// handle unicode output when the UTF-8 BOM is used. See
/// https://github.com/openai/codex/pull/7902 for more context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[test_case(true ; "with_login")]
#[test_case(false ; "without_login")]
async fn unicode_output(login: bool) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.2")).await?;

    let call_id = "unicode_output";
    let command = if cfg!(windows) {
        // We use a child process on Windows instead of a PowerShell command
        // like `Write-Output` to ensure that the Powershell config is set
        // correctly.
        "cmd.exe /c echo naïve_café"
    } else {
        "echo \"naïve_café\""
    };
    mount_shell_responses_with_timeout(&harness, call_id, command, Some(login), MEDIUM_TIMEOUT)
        .await;
    harness.submit("run the command without login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "naïve_café")?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[test_case(true ; "with_login")]
#[test_case(false ; "without_login")]
async fn unicode_output_with_newlines(login: bool) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let harness = shell_command_harness_with(|builder| builder.with_model("gpt-5.2")).await?;

    let call_id = "unicode_output";
    mount_shell_responses_with_timeout(
        &harness,
        call_id,
        "echo 'line1\nnaïve café\nline3'",
        Some(login),
        MEDIUM_TIMEOUT,
    )
    .await;
    harness.submit("run the command without login").await?;

    let output = harness.function_call_stdout(call_id).await;
    assert_shell_command_output(&output, "line1\\nnaïve café\\nline3")?;

    Ok(())
}
