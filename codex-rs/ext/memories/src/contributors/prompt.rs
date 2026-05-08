use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::PromptFragment;

use crate::ctx::read_surface_enabled;

#[derive(Debug)]
pub(crate) struct PromptContributor {
    read_prompt: Option<String>,
}

impl PromptContributor {
    pub(crate) fn new(read_prompt: Option<String>) -> Self {
        Self { read_prompt }
    }
}

impl ContextContributor for PromptContributor {
    fn contribute(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<PromptFragment> {
        if !read_surface_enabled(thread_store) {
            return Vec::new();
        }

        self.read_prompt
            .as_deref()
            .map(PromptFragment::developer_policy)
            .into_iter()
            .collect()
    }
}
