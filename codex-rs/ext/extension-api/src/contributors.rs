use std::future::Future;

use codex_protocol::items::TurnItem;

use crate::ExtensionData;

mod prompt;
mod tool;

pub use prompt::PromptFragment;
pub use prompt::PromptSlot;
pub use tool::ToolCallError;
pub use tool::ToolContribution;
pub use tool::ToolHandler;

/// Extension contribution that adds prompt fragments during prompt assembly.
pub trait ContextContributor<C>: Send + Sync {
    fn contribute(
        &self,
        context: &C,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<PromptFragment>;
}

/// Extension contribution that exposes native tools owned by a feature.
pub trait ToolContributor<C>: Send + Sync {
    /// Returns the native tools visible for the supplied runtime context.
    fn tools(&self, context: &C, thread_store: &ExtensionData) -> Vec<ToolContribution<C>>;
}

/// Future returned by one ordered turn-item contribution.
pub type TurnItemContributionFuture<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Ordered post-processing contribution for one parsed turn item.
///
/// Implementations may mutate the item before it is emitted and may use the
/// explicitly exposed thread- and turn-lifetime stores when they need durable
/// extension-private state.
pub trait TurnItemContributor<C>: Send + Sync {
    fn contribute<'a>(
        &'a self,
        context: &'a C,
        thread_store: &'a ExtensionData,
        turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> TurnItemContributionFuture<'a>;
}

// TODO: WIP (do not consider)
/// Extension contribution that can claim approval requests for a runtime context.
/// (ideally we can replace it by a session lifecycle thing or a request contributor?)
pub trait ApprovalInterceptorContributor<C>: Send + Sync {
    /// Returns whether this contributor should intercept approvals in `context`.
    fn intercepts_approvals(
        &self,
        context: &C,
        thread_store: &ExtensionData,
        turn_store: &ExtensionData,
    ) -> bool;
}
