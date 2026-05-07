//! Runtime contribution ports exposed by the extension API.
//!
//! Extensions can install several independent contribution types. This module
//! is the small public façade over those ports: callers can import the common
//! vocabulary from one place, while each contribution family keeps its own
//! supporting types nearby.

mod prompt;

pub use prompt::PromptFragment;
pub use prompt::PromptSlot;

use rmcp::model::Tool;

/// Extension contribution that adds prompt fragments during prompt assembly.
///
/// Implementations should inspect only their feature-owned slice of the
/// current runtime context and describe the prompt content exposed for that
/// invocation. The host remains responsible for ordering fragments and
/// assembling prompt items.
pub trait PromptContributor<C>: Send + Sync {
    fn contribute(&self, context: &C) -> Vec<PromptFragment>;
}

/// Extension contribution that exposes MCP tool definitions owned by a feature.
///
/// Implementations should inspect only their feature-owned slice of the
/// current runtime context and return the tools exposed for that invocation.
/// The host remains responsible for mounting those tools and routing
/// execution.
///
/// This is intentionally MCP-shaped for now because the more general tool
/// abstraction has not been extracted yet.
pub trait McpToolContributor<C>: Send + Sync {
    fn tools(&self, context: &C) -> Vec<Tool>;
}
