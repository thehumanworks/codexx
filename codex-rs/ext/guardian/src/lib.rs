//! Guardian routing contribution packaged as a Codex extension.

#![forbid(unsafe_code)]

use std::sync::Arc;

use codex_extension_api::ApprovalInterceptorContributor;
use codex_extension_api::CodexExtension;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptFragment;

/// Runtime facts needed to expose Guardian surfaces.
///
/// Hosts should provide the effective approval settings for the current turn,
/// whether the current session is the Guardian reviewer itself, and the prompt
/// text to show to that reviewer.
pub trait GuardianContext {
    fn automatic_review_enabled(&self) -> bool;
    fn approval_policy_allows_automatic_review(&self) -> bool;
    fn is_guardian_reviewer(&self) -> bool;
    fn guardian_policy_prompt(&self) -> Option<&str>;
}

/// Extension that contributes Guardian approval routing and reviewer policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct GuardianExtension;

impl GuardianExtension {
    /// Creates an extension instance.
    pub fn new() -> Self {
        Self
    }

    /// Returns whether Guardian should intercept approvals in this context.
    pub fn should_intercept_approvals<C: GuardianContext>(&self, context: &C) -> bool {
        context.automatic_review_enabled() && context.approval_policy_allows_automatic_review()
    }

    /// Returns the policy prompt shown only to Guardian reviewer sessions.
    pub fn policy_prompt<'a, C: GuardianContext>(&self, context: &'a C) -> Option<&'a str> {
        if context.is_guardian_reviewer() {
            context.guardian_policy_prompt()
        } else {
            None
        }
    }
}

impl<C: GuardianContext> ApprovalInterceptorContributor<C> for GuardianExtension {
    fn intercepts_approvals(&self, context: &C) -> bool {
        self.should_intercept_approvals(context)
    }
}

impl<C: GuardianContext> ContextContributor<C> for GuardianExtension {
    fn contribute(&self, context: &C) -> Vec<PromptFragment> {
        self.policy_prompt(context)
            .map(PromptFragment::separate_developer)
            .into_iter()
            .collect()
    }
}

impl<C: GuardianContext> CodexExtension<C> for GuardianExtension {
    fn install(self: Arc<Self>, registry: &mut ExtensionRegistryBuilder<C>) {
        registry.approval_interceptor_contributor(self.clone());
        registry.prompt_contributor(self);
    }
}

/// Creates a shared Guardian extension instance.
pub fn extension() -> Arc<GuardianExtension> {
    Arc::new(GuardianExtension::new())
}
