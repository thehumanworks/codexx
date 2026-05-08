use std::sync::Arc;

use codex_core::ExtensionContext;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;

pub(crate) fn thread_extensions() -> Arc<ExtensionRegistry<ExtensionContext>> {
    Arc::new(
        ExtensionRegistryBuilder::<ExtensionContext>::new()
            .with_extension(codex_git_attribution::extension())
            .build(),
    )
}
