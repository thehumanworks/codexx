//! Runtime contribution ports exposed by the extension API.
//!
//! Extensions can install several independent contribution types. This module
//! is the small public façade over those ports: callers can import the common
//! vocabulary from one place, while each contribution family keeps its own
//! supporting types nearby.

use std::future::Future;

mod prompt;
mod session_lifecycle;
mod tool;

pub use prompt::PromptFragment;
pub use prompt::PromptSlot;
pub use tool::ToolCallError;
pub use tool::ToolContribution;
pub use tool::ToolHandler;

/// Extension contribution that adds prompt fragments during prompt assembly.
/// Arguably, can become async
pub trait ContextContributor<C>: Send + Sync {
    fn contribute(&self, context: &C) -> Vec<PromptFragment>; // TODO use existing fragments ofc
}

/// Extension contribution that exposes native tools owned by a feature.
pub trait ToolContributor<C>: Send + Sync {
    /// Returns the native tools visible for the supplied runtime context.
    fn tools(&self, context: &C) -> Vec<ToolContribution<C>>;
}

/// Future returned by one ordered output contribution.
pub type OutputContributionFuture<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Ordered post-processing contribution for one completed output value.
///
/// Implementations may inspect or mutate `output`; hosts are expected to run
/// contributors sequentially so each contributor observes the result of the
/// previous one.
pub trait OutputContributor<C, O>: Send + Sync {
    fn contribute<'a>(&'a self, context: &'a C, output: &'a mut O) -> OutputContributionFuture<'a>;
}

// TODO: WIP
/// Extension contribution that can claim approval requests for a runtime context.
/// (ideally we can replace it by a session lifecycle thing or a request contributor?)
pub trait ApprovalInterceptorContributor<C>: Send + Sync {
    /// Returns whether this contributor should intercept approvals in `context`.
    fn intercepts_approvals(&self, context: &C) -> bool;
}

pub trait SessionLifecycleContributor<S, T>: Send + Sync {
    fn on_lifecycle_event(
        &self,
        event: session_lifecycle::Event<'_, S, T>,
    ) -> impl Future<Output = ()> + Send;
}
