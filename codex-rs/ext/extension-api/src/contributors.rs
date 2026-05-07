use std::future::Future;

use crate::Stores;

mod prompt;
mod tool;

pub use prompt::PromptFragment;
pub use prompt::PromptSlot;
pub use tool::ToolCallError;
pub use tool::ToolContribution;
pub use tool::ToolHandler;

/// Extension contribution that adds prompt fragments during prompt assembly.
pub trait ContextContributor<C>: Send + Sync {
    fn contribute(&self, context: &C, stores: &Stores<'_>) -> Vec<PromptFragment>;
}

/// Extension contribution that exposes native tools owned by a feature.
pub trait ToolContributor<C>: Send + Sync {
    /// Returns the native tools visible for the supplied runtime context.
    fn tools(&self, context: &C, stores: &Stores<'_>) -> Vec<ToolContribution<C>>;
}

/// Future returned by one ordered output contribution.
pub type OutputContributionFuture<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Ordered post-processing contribution for one completed output value.
///
/// This is kept abstract so that we can re-use it at multiple places. I kind of like it but I understand
/// if this is problematic with bad devs
pub trait OutputContributor<C, O>: Send + Sync {
    fn contribute<'a>(
        &'a self,
        context: &'a C,
        stores: &'a Stores<'a>,
        output: &'a mut O,
    ) -> OutputContributionFuture<'a>;
}

// TODO: WIP (do not consider)
/// Extension contribution that can claim approval requests for a runtime context.
/// (ideally we can replace it by a session lifecycle thing or a request contributor?)
pub trait ApprovalInterceptorContributor<C>: Send + Sync {
    /// Returns whether this contributor should intercept approvals in `context`.
    fn intercepts_approvals(&self, context: &C, stores: &Stores<'_>) -> bool;
}
