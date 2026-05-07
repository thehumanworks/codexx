/// Runtime facts needed to decide whether read-memory surfaces are visible.
///
/// Hosts should expose the current effective values for the thread being
/// assembled. The extension owns the policy that combines those values.
pub trait MemoriesContext {
    fn memory_tool_enabled(&self) -> bool;
    fn use_memories(&self) -> bool;
}
