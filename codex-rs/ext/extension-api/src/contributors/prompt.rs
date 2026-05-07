/// Prompt region targeted by a contributed fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptSlot {
    DeveloperPolicy,
    DeveloperCapabilities,
    ContextualUser,
    SeparateDeveloper,
}

/// One text fragment contributed to prompt assembly by an extension. (wiring not done yet on purpose)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptFragment {
    slot: PromptSlot,
    text: String,
}

impl PromptFragment {
    /// Creates a prompt fragment for the given slot.
    pub fn new(slot: PromptSlot, text: impl Into<String>) -> Self {
        Self {
            slot,
            text: text.into(),
        }
    }

    /// Creates a developer-policy prompt fragment.
    pub fn developer_policy(text: impl Into<String>) -> Self {
        Self::new(PromptSlot::DeveloperPolicy, text)
    }

    /// Creates a developer-capabilities prompt fragment.
    pub fn developer_capability(text: impl Into<String>) -> Self {
        Self::new(PromptSlot::DeveloperCapabilities, text)
    }

    /// Creates a separate top-level developer prompt fragment.
    pub fn separate_developer(text: impl Into<String>) -> Self {
        Self::new(PromptSlot::SeparateDeveloper, text)
    }

    /// Returns the target prompt slot.
    pub fn slot(&self) -> PromptSlot {
        self.slot
    }

    /// Returns the model-visible text.
    pub fn text(&self) -> &str {
        &self.text
    }
}
