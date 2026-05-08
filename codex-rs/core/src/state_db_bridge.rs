use codex_rollout::state_db as rollout_state_db;
pub use codex_rollout::state_db::StateDbAccess;
pub use codex_rollout::state_db::StateDbHandle;
pub use codex_state::DbMetricsRecorderHandle;

use crate::config::Config;

pub async fn init_state_db(
    config: &Config,
    metrics: Option<codex_state::DbMetricsRecorderHandle>,
) -> Option<StateDbHandle> {
    rollout_state_db::init(config, metrics).await
}
