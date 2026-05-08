use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use crate::DB_FALLBACK_METRIC;
use crate::DB_INIT_DURATION_METRIC;
use crate::DB_INIT_METRIC;

/// Low-cardinality metrics sink used by the SQLite state runtime.
///
/// Implementations should ignore recording errors locally. Database operations
/// must never fail because telemetry delivery failed.
pub trait DbMetricsRecorder: Send + Sync + 'static {
    /// Increment a counter metric by `inc` with low-cardinality tags.
    fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]);

    /// Record an elapsed duration metric with low-cardinality tags.
    fn record_duration(&self, name: &str, duration: Duration, tags: &[(&str, &str)]);
}

/// Shared recorder handle stored by rollout SQLite telemetry plumbing.
pub type DbMetricsRecorderHandle = Arc<dyn DbMetricsRecorder>;

#[derive(Clone, Copy)]
pub(crate) enum DbKind {
    State,
    Logs,
}

impl DbKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Logs => "logs",
        }
    }
}

pub(crate) fn record_init_result<T>(
    metrics: Option<&dyn DbMetricsRecorder>,
    db: DbKind,
    phase: &'static str,
    duration: Duration,
    result: &anyhow::Result<T>,
) {
    let outcome = DbOutcomeTags::from_result(result);
    let tags = [
        ("status", outcome.status),
        ("phase", phase),
        ("db", db.as_str()),
        ("error", outcome.error),
    ];
    record_counter(metrics, DB_INIT_METRIC, &tags);
    record_duration(metrics, DB_INIT_DURATION_METRIC, duration, &tags);
}

pub fn record_fallback(
    metrics: Option<&dyn DbMetricsRecorder>,
    caller: &'static str,
    reason: &'static str,
) {
    let tags = [("caller", caller), ("reason", reason)];
    record_counter(metrics, DB_FALLBACK_METRIC, &tags);
}

pub fn record_init_backfill_gate(
    metrics: Option<&dyn DbMetricsRecorder>,
    duration: Duration,
    result: &anyhow::Result<()>,
) {
    record_init_result(metrics, DbKind::State, "backfill_gate", duration, result);
}

pub(crate) fn classify_error(err: &anyhow::Error) -> &'static str {
    for cause in err.chain() {
        if let Some(sqlx_err) = cause.downcast_ref::<sqlx::Error>() {
            return classify_sqlx_error(sqlx_err);
        }
        if cause
            .downcast_ref::<sqlx::migrate::MigrateError>()
            .is_some()
        {
            return "migration";
        }
        if cause.downcast_ref::<serde_json::Error>().is_some() {
            return "serde";
        }
        if cause.downcast_ref::<std::io::Error>().is_some() {
            return "io";
        }
    }

    "unknown"
}

pub(crate) fn classify_sqlite_code(code: &str) -> &'static str {
    // SQLite result codes are documented at https://www.sqlite.org/rescode.html.
    // Extended codes preserve the primary code in the low byte.
    let primary_code = code.parse::<i32>().ok().map(|code| code & 0xff);
    match primary_code {
        Some(5) => "busy",
        Some(6) => "locked",
        Some(8) => "readonly",
        Some(10) => "io",
        Some(11) => "corrupt",
        Some(13) => "full",
        Some(14) => "cantopen",
        Some(19) => "constraint",
        Some(17) => "schema",
        _ => "unknown",
    }
}

struct DbOutcomeTags {
    status: &'static str,
    error: &'static str,
}

impl DbOutcomeTags {
    fn from_result<T>(result: &anyhow::Result<T>) -> Self {
        match result {
            Ok(_) => Self {
                status: "success",
                error: "none",
            },
            Err(err) => Self {
                status: "failed",
                error: classify_error(err),
            },
        }
    }
}

fn classify_sqlx_error(err: &sqlx::Error) -> &'static str {
    match err {
        sqlx::Error::Database(database_error) => {
            let code = database_error
                .code()
                .unwrap_or(Cow::Borrowed("none"))
                .to_string();
            classify_sqlite_code(code.as_str())
        }
        sqlx::Error::PoolTimedOut => "pool_timeout",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::ColumnDecode { source, .. } if source.is::<serde_json::Error>() => "serde",
        sqlx::Error::Decode(source) if source.is::<serde_json::Error>() => "serde",
        _ => "unknown",
    }
}

fn record_counter(metrics: Option<&dyn DbMetricsRecorder>, name: &str, tags: &[(&str, &str)]) {
    if let Some(metrics) = metrics {
        metrics.counter(name, /*inc*/ 1, tags);
    }
}

fn record_duration(
    metrics: Option<&dyn DbMetricsRecorder>,
    name: &str,
    duration: Duration,
    tags: &[(&str, &str)],
) {
    if let Some(metrics) = metrics {
        metrics.record_duration(name, duration, tags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DB_FALLBACK_METRIC;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestMetrics {
        events: Mutex<Vec<MetricEvent>>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct MetricEvent {
        name: String,
        tags: BTreeMap<String, String>,
    }

    impl TestMetrics {
        fn events(&self) -> Vec<MetricEvent> {
            self.events
                .lock()
                .expect("metrics lock")
                .iter()
                .map(|event| MetricEvent {
                    name: event.name.clone(),
                    tags: event.tags.clone(),
                })
                .collect()
        }
    }

    impl DbMetricsRecorder for TestMetrics {
        fn counter(&self, name: &str, _inc: i64, tags: &[(&str, &str)]) {
            self.events.lock().expect("metrics lock").push(MetricEvent {
                name: name.to_string(),
                tags: tags_to_map(tags),
            });
        }

        fn record_duration(&self, _name: &str, _duration: Duration, _tags: &[(&str, &str)]) {}
    }

    fn tags_to_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn classifies_sqlite_primary_codes() {
        assert_eq!(classify_sqlite_code("5"), "busy");
        assert_eq!(classify_sqlite_code("6"), "locked");
        assert_eq!(classify_sqlite_code("14"), "cantopen");
        assert_eq!(classify_sqlite_code("2067"), "constraint");
    }

    #[test]
    fn classifies_non_sqlite_errors() {
        let io_error =
            anyhow::Error::new(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
        assert_eq!(classify_error(&io_error), "io");

        let serde_error =
            anyhow::Error::new(serde_json::from_str::<serde_json::Value>("not-json").unwrap_err());
        assert_eq!(classify_error(&serde_error), "serde");

        let unknown_error = anyhow::anyhow!("plain failure");
        assert_eq!(classify_error(&unknown_error), "unknown");
    }

    #[test]
    fn classifies_sqlx_pool_timeout() {
        let err = anyhow::Error::new(sqlx::Error::PoolTimedOut);
        assert_eq!(classify_error(&err), "pool_timeout");
    }

    #[test]
    fn records_fallback_metric_with_reason() {
        let metrics = TestMetrics::default();

        record_fallback(Some(&metrics), "list_threads", "db_error");

        assert_eq!(
            metrics.events(),
            vec![MetricEvent {
                name: DB_FALLBACK_METRIC.to_string(),
                tags: BTreeMap::from([
                    ("caller".to_string(), "list_threads".to_string()),
                    ("reason".to_string(), "db_error".to_string()),
                ]),
            }]
        );
    }
}
