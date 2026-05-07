use anyhow::Result;
use codex_protocol::ThreadId;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadQueuedTurn {
    pub queued_turn_id: String,
    pub thread_id: ThreadId,
    pub turn_start_params_json: String,
    pub queue_order: i64,
}

pub(crate) struct ThreadQueuedTurnRow {
    pub queued_turn_id: String,
    pub thread_id: String,
    pub turn_start_params_json: String,
    pub queue_order: i64,
}

impl ThreadQueuedTurnRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            queued_turn_id: row.try_get("queued_turn_id")?,
            thread_id: row.try_get("thread_id")?,
            turn_start_params_json: row.try_get("turn_start_params_json")?,
            queue_order: row.try_get("queue_order")?,
        })
    }
}

impl TryFrom<ThreadQueuedTurnRow> for ThreadQueuedTurn {
    type Error = anyhow::Error;

    fn try_from(row: ThreadQueuedTurnRow) -> Result<Self> {
        Ok(Self {
            queued_turn_id: row.queued_turn_id,
            thread_id: ThreadId::try_from(row.thread_id)?,
            turn_start_params_json: row.turn_start_params_json,
            queue_order: row.queue_order,
        })
    }
}
