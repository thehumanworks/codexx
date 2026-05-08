use std::sync::Arc;
use std::time::Duration;

use codex_otel::ORIGINATOR_TAG;
use codex_otel::bounded_originator_tag_value;
use codex_state::DbMetricsRecorder;
use codex_state::DbMetricsRecorderHandle;

struct OtelDbMetrics {
    metrics: codex_otel::MetricsClient,
    originator: &'static str,
}

impl DbMetricsRecorder for OtelDbMetrics {
    fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]) {
        let tags = sqlite_originator_tags(tags, self.originator);
        let _ = self.metrics.counter(name, inc, &tags);
    }

    fn record_duration(&self, name: &str, duration: Duration, tags: &[(&str, &str)]) {
        let tags = sqlite_originator_tags(tags, self.originator);
        let _ = self.metrics.record_duration(name, duration, &tags);
    }
}

pub(crate) fn recorder(
    metrics: codex_otel::MetricsClient,
    originator: &str,
) -> DbMetricsRecorderHandle {
    Arc::new(OtelDbMetrics {
        metrics,
        originator: bounded_originator_tag_value(originator),
    })
}

fn sqlite_originator_tags<'a>(
    tags: &[(&'a str, &'a str)],
    originator: &'static str,
) -> Vec<(&'a str, &'a str)> {
    let mut tags = tags.to_vec();
    tags.push((ORIGINATOR_TAG, originator));
    tags
}
