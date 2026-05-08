use codex_extension_api::ExtensionData;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MemoriesReadConfig {
    pub(crate) enabled: bool,
}

pub(crate) fn read_surface_enabled(thread_store: &ExtensionData) -> bool {
    thread_store
        .get::<MemoriesReadConfig>()
        .is_some_and(|config| config.enabled)
}
