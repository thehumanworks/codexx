//! Git-attribution prompt contribution packaged as a Codex extension.

#![forbid(unsafe_code)]

use std::sync::Arc;

use codex_extension_api::CodexExtension;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptContributor;
use codex_extension_api::PromptFragment;

const DEFAULT_ATTRIBUTION_VALUE: &str = "Codex <noreply@openai.com>";

/// Runtime facts needed to render the git-attribution prompt contribution.
///
/// Hosts should return the current effective attribution override for the
/// thread being assembled. `None` selects Codex's default attribution, while a
/// blank string disables the contribution.
pub trait GitAttributionContext {
    fn commit_attribution(&self) -> Option<&str>;
}

/// Prompt-only extension that contributes the configured git-attribution instruction.
#[derive(Clone, Copy, Debug, Default)]
pub struct GitAttributionExtension;

impl GitAttributionExtension {
    /// Creates an extension instance.
    pub fn new() -> Self {
        Self
    }

    /// Returns the model-visible trailer instruction, if attribution is enabled.
    pub fn instruction<C: GitAttributionContext>(&self, context: &C) -> Option<String> {
        let trailer = build_commit_message_trailer(context.commit_attribution())?;
        Some(format!(
            "When you write or edit a git commit message, ensure the message ends with this trailer exactly once:\n{trailer}\n\nRules:\n- Keep existing trailers and append this trailer at the end if missing.\n- Do not duplicate this trailer if it already exists.\n- Keep one blank line between the commit body and trailer block."
        ))
    }
}

impl<C: GitAttributionContext> PromptContributor<C> for GitAttributionExtension {
    fn contribute(&self, context: &C) -> Vec<PromptFragment> {
        self.instruction(context)
            .map(PromptFragment::developer_capability)
            .into_iter()
            .collect()
    }
}

impl<C: GitAttributionContext> CodexExtension<C> for GitAttributionExtension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.prompt_contributor(self);
    }
}

/// Creates a shared git-attribution extension instance.
pub fn extension() -> Arc<GitAttributionExtension> {
    Arc::new(GitAttributionExtension::new())
}

fn build_commit_message_trailer(config_attribution: Option<&str>) -> Option<String> {
    let value = resolve_attribution_value(config_attribution)?;
    Some(format!("Co-authored-by: {value}"))
}

fn resolve_attribution_value(config_attribution: Option<&str>) -> Option<String> {
    match config_attribution {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => Some(DEFAULT_ATTRIBUTION_VALUE.to_string()),
    }
}
