//! MultiAgentV2 usage-hint prompt contribution packaged as a Codex extension.

#![forbid(unsafe_code)]

use std::sync::Arc;

use codex_extension_api::CodexExtension;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptContributor;
use codex_extension_api::PromptFragment;

/// Which kind of agent should receive a MultiAgentV2 usage hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageHintAudience {
    RootAgent,
    SpawnedSubagent,
    Omitted,
}

/// Runtime facts needed to render MultiAgentV2 usage-hint contributions.
///
/// Hosts should return the current effective settings for the thread being
/// assembled. The extension owns the routing policy that chooses which hint, if
/// any, is visible for that invocation.
pub trait MultiAgentV2Context {
    fn multi_agent_v2_enabled(&self) -> bool;
    fn usage_hint_audience(&self) -> UsageHintAudience;
    fn root_agent_usage_hint_text(&self) -> Option<&str>;
    fn subagent_usage_hint_text(&self) -> Option<&str>;
}

/// Prompt-only extension that contributes one MultiAgentV2 usage hint when enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct MultiAgentV2Extension;

impl MultiAgentV2Extension {
    /// Creates an extension instance.
    pub fn new() -> Self {
        Self
    }

    /// Returns the usage hint text that should be shown for this agent, if any.
    pub fn usage_hint_text<'a, C: MultiAgentV2Context>(&self, context: &'a C) -> Option<&'a str> {
        if !context.multi_agent_v2_enabled() {
            return None;
        }

        match context.usage_hint_audience() {
            UsageHintAudience::RootAgent => context.root_agent_usage_hint_text(),
            UsageHintAudience::SpawnedSubagent => context.subagent_usage_hint_text(),
            UsageHintAudience::Omitted => None,
        }
    }
}

impl<C: MultiAgentV2Context> PromptContributor<C> for MultiAgentV2Extension {
    fn contribute(&self, context: &C) -> Vec<PromptFragment> {
        self.usage_hint_text(context)
            .map(PromptFragment::separate_developer)
            .into_iter()
            .collect()
    }
}

impl<C: MultiAgentV2Context> CodexExtension<C> for MultiAgentV2Extension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.prompt_contributor(self);
    }
}

/// Creates a shared MultiAgentV2 extension instance.
pub fn extension() -> Arc<MultiAgentV2Extension> {
    Arc::new(MultiAgentV2Extension::new())
}
