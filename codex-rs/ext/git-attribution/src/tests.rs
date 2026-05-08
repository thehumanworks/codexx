use super::GitAttributionContext;
use super::GitAttributionExtension;
use super::build_commit_message_trailer;
use super::resolve_attribution_value;

struct TestContext {
    git_attribution_enabled: bool,
    commit_attribution: Option<&'static str>,
}

impl GitAttributionContext for TestContext {
    fn git_attribution_enabled(&self) -> bool {
        self.git_attribution_enabled
    }

    fn commit_attribution(&self) -> Option<&str> {
        self.commit_attribution
    }
}

#[test]
fn blank_attribution_disables_trailer_prompt() {
    assert_eq!(build_commit_message_trailer(Some("")), None);
    assert_eq!(
        GitAttributionExtension::new().instruction(&TestContext {
            git_attribution_enabled: true,
            commit_attribution: Some("   "),
        }),
        None
    );
}

#[test]
fn disabled_context_omits_instruction() {
    assert_eq!(
        GitAttributionExtension::new().instruction(&TestContext {
            git_attribution_enabled: false,
            commit_attribution: None,
        }),
        None
    );
}

#[test]
fn default_attribution_uses_codex_trailer() {
    assert_eq!(
        build_commit_message_trailer(/*config_attribution*/ None).as_deref(),
        Some("Co-authored-by: Codex <noreply@openai.com>")
    );
}

#[test]
fn resolve_value_handles_default_custom_and_blank() {
    assert_eq!(
        resolve_attribution_value(/*config_attribution*/ None),
        Some("Codex <noreply@openai.com>".to_string())
    );
    assert_eq!(
        resolve_attribution_value(Some("MyAgent <me@example.com>")),
        Some("MyAgent <me@example.com>".to_string())
    );
    assert_eq!(
        resolve_attribution_value(Some("MyAgent")),
        Some("MyAgent".to_string())
    );
    assert_eq!(resolve_attribution_value(Some("   ")), None);
}

#[test]
fn instruction_mentions_trailer_and_omits_generated_with() {
    let instruction = GitAttributionExtension::new()
        .instruction(&TestContext {
            git_attribution_enabled: true,
            commit_attribution: Some("AgentX <agent@example.com>"),
        })
        .expect("instruction expected");

    assert!(instruction.contains("Co-authored-by: AgentX <agent@example.com>"));
    assert!(instruction.contains("exactly once"));
    assert!(!instruction.contains("Generated-with"));
}
