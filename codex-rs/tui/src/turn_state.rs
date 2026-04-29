//! Turn lifecycle state used by TUI interruption and completion handling.

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TurnAbortReason {
    Interrupted,
    BudgetLimited,
}
