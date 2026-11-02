//! Per engine runtime state: suspensions and statistics, ported from the
//! error handling in `searx/engines/__init__.py` (`ban_time_on_fail`,
//! `max_ban_time_on_fail` and `suspended_times`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::FailureKind;
use crate::settings::Search;

/// Counters kept per engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineStats {
    pub requests: u64,
    pub results: u64,
    pub errors: u64,
    pub timeouts: u64,
    pub http_errors: u64,
    pub total_time: f64,
}

impl EngineStats {
    /// Average response time in seconds.
    pub fn average_time(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.total_time / self.requests as f64
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EngineState {
    consecutive_failures: u64,
    /// Successful answers that carried no results. An engine that keeps
    /// answering nothing is costing every search its timeout for free.
    empty_streak: u64,
    /// When the engine may rejoin the rotation.
    quiet_until: Option<Instant>,
    suspended_until: Option<Instant>,
    suspension_reason: Option<String>,
    stats: EngineStats,
}

/// A read-only view of one engine's state, used by `/stats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSnapshot {
    pub engine: String,
    pub stats: EngineStats,
    pub consecutive_failures: u64,
    pub suspended: bool,
    pub suspended_for: f64,
    pub suspension_reason: Option<String>,
}

/// Thread safe map of engine states.
#[derive(Debug, Default)]
pub struct EngineStates {
    states: Mutex<HashMap<String, EngineState>>,
}

impl EngineStates {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_state<T>(&self, engine: &str, action: impl FnOnce(&mut EngineState) -> T) -> T {
        let mut states = self.states.lock().expect("engine states are not poisoned");
        let state = states.entry(engine.to_string()).or_default();
        action(state)
    }

    /// Remaining suspension time, if the engine is suspended.
    pub fn suspension(&self, engine: &str) -> Option<(Duration, Option<String>)> {
        self.with_state(engine, |state| {
            // An engine that keeps answering nothing is rested, not failed.
            if let Some(until) = state.quiet_until {
                let now = Instant::now();
                if until > now {
                    return Some((until - now, Some("resting".to_string())));
                }
                state.quiet_until = None;
            }
            state.suspended_until.and_then(|until| {
                let now = Instant::now();
                if until > now {
                    Some((until - now, state.suspension_reason.clone()))
                } else {
                    state.suspended_until = None;
                    state.suspension_reason = None;
                    None
                }
            })
        })
    }

    /// Record a successful request.
    pub fn record_success(
        &self,
        engine: &str,
        results: usize,
        elapsed: Duration,
        settings: &Search,
    ) {
        self.with_state(engine, |state| {
            state.consecutive_failures = 0;
            if results == 0 {
                state.empty_streak += 1;
                if settings.empty_result_limit > 0
                    && state.empty_streak >= settings.empty_result_limit
                {
                    state.quiet_until = Some(
                        Instant::now() + Duration::from_secs(settings.empty_result_cooldown.max(1)),
                    );
                    state.empty_streak = 0;
                }
            } else {
                state.empty_streak = 0;
                state.quiet_until = None;
            }
            state.suspended_until = None;
            state.suspension_reason = None;
            state.stats.requests += 1;
            state.stats.results += results as u64;
            state.stats.total_time += elapsed.as_secs_f64();
        });
    }

    /// Record a failure and return the suspension time in seconds (0 = none).
    pub fn record_failure(&self, engine: &str, kind: FailureKind, settings: &Search) -> u64 {
        self.with_state(engine, |state| {
            state.stats.errors += 1;
            if kind == FailureKind::Timeout {
                state.stats.timeouts += 1;
            }
            if kind == FailureKind::Http {
                state.stats.http_errors += 1;
            }
            state.consecutive_failures += 1;

            let suspension = match kind.suspension_key() {
                Some(key) => settings.suspended_times.get(key).copied().unwrap_or(3_600),
                None => {
                    // Repeated soft failures degrade into a short suspension,
                    // exactly like upstream's ban_time_on_fail.
                    if state.consecutive_failures >= 2 {
                        (state.consecutive_failures * settings.ban_time_on_fail)
                            .min(settings.max_ban_time_on_fail)
                    } else {
                        0
                    }
                }
            };
            if suspension > 0 {
                state.suspended_until = Some(Instant::now() + Duration::from_secs(suspension));
                state.suspension_reason = Some(kind.error_type().to_string());
            }
            suspension
        })
    }

    /// Reset the counters (used by tests and by `/stats` reset).
    pub fn reset(&self) {
        self.states
            .lock()
            .expect("engine states are not poisoned")
            .clear();
    }

    /// Snapshot of every engine that has been used.
    pub fn snapshot(&self) -> Vec<EngineSnapshot> {
        let now = Instant::now();
        let states = self.states.lock().expect("engine states are not poisoned");
        let mut snapshots: Vec<EngineSnapshot> = states
            .iter()
            .map(|(engine, state)| {
                let (suspended, suspended_for) = match state.suspended_until {
                    Some(until) if until > now => (true, (until - now).as_secs_f64()),
                    _ => (false, 0.0),
                };
                EngineSnapshot {
                    engine: engine.clone(),
                    stats: state.stats.clone(),
                    consecutive_failures: state.consecutive_failures,
                    suspended,
                    suspended_for,
                    suspension_reason: state.suspension_reason.clone(),
                }
            })
            .collect();
        snapshots.sort_by(|a, b| a.engine.cmp(&b.engine));
        snapshots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_suspends_for_a_day() {
        let states = EngineStates::new();
        let settings = Search::default();
        let seconds = states.record_failure("google", FailureKind::AccessDenied, &settings);
        assert_eq!(seconds, 86_400);
        assert!(states.suspension("google").is_some());
    }

    #[test]
    fn two_soft_failures_trigger_a_short_suspension() {
        let states = EngineStates::new();
        let settings = Search::default();
        assert_eq!(states.record_failure("x", FailureKind::Other, &settings), 0);
        let second = states.record_failure("x", FailureKind::Other, &settings);
        assert_eq!(second, 10);
        assert!(states.suspension("x").is_some());
    }

    #[test]
    fn success_clears_the_state() {
        let states = EngineStates::new();
        let settings = Search::default();
        states.record_failure("x", FailureKind::TooManyRequests, &settings);
        states.record_success("x", 3, Duration::from_millis(100), &Search::default());
        assert!(states.suspension("x").is_none());
        let snapshot = states.snapshot();
        assert_eq!(snapshot[0].stats.results, 3);
        assert_eq!(snapshot[0].consecutive_failures, 0);
    }
}
