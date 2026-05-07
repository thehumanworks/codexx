use codex_extension_api::Stores;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolContributor;

use crate::MemoriesExtension;
use crate::ctx::MemoriesContext;

impl<C: MemoriesContext + Send + Sync + 'static> ToolContributor<C> for MemoriesExtension {
    fn tools(&self, context: &C, _stores: &Stores<'_>) -> Vec<ToolContribution<C>> {
        if !self.is_read_surface_enabled(context) {
            return Vec::new();
        }

        // TODO(jif) add more tools ofc
        vec![self.list_tool.contribution()]
    }
}
