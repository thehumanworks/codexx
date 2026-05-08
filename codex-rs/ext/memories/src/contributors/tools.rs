use codex_extension_api::ExtensionData;
use codex_extension_api::ToolContribution;
use codex_extension_api::ToolContributor;

use crate::ctx::read_surface_enabled;
use crate::tools::MemoriesTools;

#[derive(Debug)]
pub(crate) struct ToolsContributor {
    tools: MemoriesTools,
}

impl ToolsContributor {
    pub(crate) fn new(tools: MemoriesTools) -> Self {
        Self { tools }
    }
}

impl ToolContributor for ToolsContributor {
    fn tools(&self, thread_store: &ExtensionData) -> Vec<ToolContribution> {
        if !read_surface_enabled(thread_store) {
            return Vec::new();
        }

        self.tools.contributions()
    }
}
