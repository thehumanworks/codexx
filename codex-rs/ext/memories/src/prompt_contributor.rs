use codex_extension_api::ContextContributor;
use codex_extension_api::PromptFragment;
use codex_extension_api::Stores;

use crate::MemoriesExtension;
use crate::ctx::MemoriesContext;

impl<C: MemoriesContext> ContextContributor<C> for MemoriesExtension {
    fn contribute(&self, context: &C, _stores: &Stores<'_>) -> Vec<PromptFragment> {
        if !self.is_read_surface_enabled(context) {
            return Vec::new();
        }

        self.read_prompt()
            .map(PromptFragment::developer_policy)
            .into_iter()
            .collect()
    }
}
