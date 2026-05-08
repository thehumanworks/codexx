use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use sqlx::SqlitePool;

use crate::telemetry::DbAccess;
use crate::telemetry::DbKind;
use crate::telemetry::DbMetricsRecorder;
use crate::telemetry::DbMetricsRecorderHandle;

/// SQLite pool plus Codex-level operation telemetry context.
#[derive(Clone)]
pub(super) struct InstrumentedDb {
    pool: Arc<SqlitePool>,
    kind: DbKind,
    metrics: Option<DbMetricsRecorderHandle>,
}

impl InstrumentedDb {
    pub(super) fn new(
        pool: Arc<SqlitePool>,
        kind: DbKind,
        metrics: Option<DbMetricsRecorderHandle>,
    ) -> Self {
        Self {
            pool,
            kind,
            metrics,
        }
    }

    pub(super) fn pool(&self) -> &SqlitePool {
        self.pool.as_ref()
    }

    pub(super) fn metrics(&self) -> Option<&dyn DbMetricsRecorder> {
        self.metrics.as_deref()
    }

    pub(super) fn metrics_handle(&self) -> Option<DbMetricsRecorderHandle> {
        self.metrics.clone()
    }

    pub(super) async fn read<T, F, Fut>(&self, operation: DbOperation, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.record_operation(operation, DbAccess::Read, f).await
    }

    pub(super) async fn write<T, F, Fut>(&self, operation: DbOperation, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.record_operation(operation, DbAccess::Write, f).await
    }

    pub(super) async fn transaction<T, F, Fut>(
        &self,
        operation: DbOperation,
        f: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.record_operation(operation, DbAccess::Transaction, f)
            .await
    }

    pub(super) async fn maintenance<T, F, Fut>(
        &self,
        operation: DbOperation,
        f: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.record_operation(operation, DbAccess::Maintenance, f)
            .await
    }

    pub(super) fn record_result<T>(
        &self,
        operation: DbOperation,
        access: DbAccess,
        started: Instant,
        result: &anyhow::Result<T>,
    ) {
        crate::telemetry::record_operation_result(
            self.metrics(),
            self.kind,
            operation.as_str(),
            access,
            started.elapsed(),
            result,
        );
    }

    async fn record_operation<T, F, Fut>(
        &self,
        operation: DbOperation,
        access: DbAccess,
        f: F,
    ) -> anyhow::Result<T>
    where
        F: FnOnce(SqlitePool) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let started = Instant::now();
        let result = f(self.pool().clone()).await;
        self.record_result(operation, access, started, &result);
        result
    }
}

#[derive(Clone, Copy)]
pub(super) enum DbOperation {
    CheckpointBackfill,
    FindRolloutPathById,
    GetBackfillState,
    GetDynamicTools,
    GetThread,
    InsertLogs,
    ListThreads,
    LogsStartupMaintenance,
    MarkBackfillComplete,
    MarkBackfillRunning,
    PersistDynamicTools,
    TouchThreadUpdatedAt,
    TryClaimBackfill,
    UpsertThread,
}

impl DbOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::CheckpointBackfill => "checkpoint_backfill",
            Self::FindRolloutPathById => "find_rollout_path_by_id",
            Self::GetBackfillState => "get_backfill_state",
            Self::GetDynamicTools => "get_dynamic_tools",
            Self::GetThread => "get_thread",
            Self::InsertLogs => "insert_logs",
            Self::ListThreads => "list_threads",
            Self::LogsStartupMaintenance => "logs_startup_maintenance",
            Self::MarkBackfillComplete => "mark_backfill_complete",
            Self::MarkBackfillRunning => "mark_backfill_running",
            Self::PersistDynamicTools => "persist_dynamic_tools",
            Self::TouchThreadUpdatedAt => "touch_thread_updated_at",
            Self::TryClaimBackfill => "try_claim_backfill",
            Self::UpsertThread => "upsert_thread",
        }
    }
}
