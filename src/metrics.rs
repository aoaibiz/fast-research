//! Instance wide counters, ported from `searx/metrics/`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Counters incremented by the searcher and the web layer.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    pub searches: AtomicU64,
    pub engine_requests: AtomicU64,
    pub engine_errors: AtomicU64,
    pub engine_timeouts: AtomicU64,
    pub results: AtomicU64,
    pub cache_hits: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            searches: AtomicU64::new(0),
            engine_requests: AtomicU64::new(0),
            engine_errors: AtomicU64::new(0),
            engine_timeouts: AtomicU64::new(0),
            results: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
        }
    }

    pub fn incr(counter: &AtomicU64, amount: u64) {
        counter.fetch_add(amount, Ordering::Relaxed);
    }

    /// A serialisable snapshot, used by `/stats`.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            uptime: self.started.elapsed().as_secs_f64(),
            searches: self.searches.load(Ordering::Relaxed),
            engine_requests: self.engine_requests.load(Ordering::Relaxed),
            engine_errors: self.engine_errors.load(Ordering::Relaxed),
            engine_timeouts: self.engine_timeouts.load(Ordering::Relaxed),
            results: self.results.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub uptime: f64,
    pub searches: u64,
    pub engine_requests: u64,
    pub engine_errors: u64,
    pub engine_timeouts: u64,
    pub results: u64,
    pub cache_hits: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_reported() {
        let metrics = Metrics::new();
        Metrics::incr(&metrics.searches, 3);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.searches, 3);
        assert!(snapshot.uptime >= 0.0);
    }
}
