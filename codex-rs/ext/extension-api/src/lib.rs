//! Semantic ports and typed assembly for first-party Codex extensions.
//!
//! This crate owns the small stable assembly layer: host-independent semantic
//! ports plus the registry used to collect their implementations. Port
//! contracts should live here only once they can be expressed without pulling
//! `codex-core` internals across the boundary.

#![forbid(unsafe_code)]

mod contributors;
mod extension;
mod registry;

pub use contributors::ApprovalInterceptorContributor;
pub use contributors::PromptContributor;
pub use contributors::PromptFragment;
pub use contributors::PromptSlot;
pub use contributors::ToolCallError;
pub use contributors::ToolContribution;
pub use contributors::ToolContributor;
pub use contributors::ToolHandler;
pub use extension::CodexExtension;
pub use registry::ExtensionRegistry;
pub use registry::ExtensionRegistryBuilder;
