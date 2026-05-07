//! Runtime contribution ports exposed by the extension API.
//!
//! Extensions can install several independent contribution types. This module
//! is the small public façade over those ports: callers can import the common
//! vocabulary from one place, while each contribution family keeps its own
//! supporting types nearby.

mod prompt;
mod tool;

pub use prompt::PromptFragment;
pub use prompt::PromptSlot;
pub use tool::ToolCallError;
pub use tool::ToolContribution;
pub use tool::ToolHandler;

/// Extension contribution that can claim approval requests for a runtime context.
///
/// Implementations should make only the routing decision here. The host keeps
/// ownership of executing the chosen review flow and translating its result
/// back into the surrounding runtime.
pub trait ApprovalInterceptorContributor<C>: Send + Sync {
    /// Returns whether this contributor should intercept approvals in `context`.
    fn intercepts_approvals(&self, context: &C) -> bool;
}

/// Extension contribution that adds prompt fragments during prompt assembly.
///
/// Implementations should inspect only their feature-owned slice of the
/// current runtime context and describe the prompt content exposed for that
/// invocation. The host remains responsible for ordering fragments and
/// assembling prompt items.
pub trait PromptContributor<C>: Send + Sync {
    fn contribute(&self, context: &C) -> Vec<PromptFragment>;
}

/// Extension contribution that exposes native tools owned by a feature.
///
/// Implementations should inspect only their feature-owned slice of the
/// current runtime context and return the tools exposed for that invocation.
/// The host remains responsible for mounting those tools and adapting calls
/// into its runtime.
pub trait ToolContributor<C>: Send + Sync {
    /// Returns the native tools visible for the supplied runtime context.
    fn tools(&self, context: &C) -> Vec<ToolContribution<C>>;
}
