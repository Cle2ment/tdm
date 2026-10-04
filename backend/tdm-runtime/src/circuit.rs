//! In-memory per-provider circuit breaker (plan §5).
//!
//! Three states over one consecutive-failure counter:
//!
//! - **closed**: calls pass through.
//! - **open** (failures ≥ threshold): calls fail fast with
//!   `TdmError::Server { status: 503, message: "circuit open" }` until the
//!   cooldown elapses.
//! - **half-open** (cooldown elapsed): a single probe call is admitted;
//!   concurrent callers keep failing fast. Success closes the circuit and
//!   resets the counter; failure re-opens it for a full cooldown.
//!
//! Only [`TdmError::Network`] / [`TdmError::Server`] outcomes count as
//! failures. Other outcomes leave the counter untouched but complete an
//! in-flight probe. State is in-memory for M1; persistence comes later.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tdm_core::TdmError;

/// Fast-fail error served while a circuit is open.
fn circuit_open_error() -> TdmError {
    TdmError::Server {
        status: 503,
        message: "circuit open".to_owned(),
    }
}

#[derive(Debug, Default)]
struct CircuitState {
    consecutive_failures: u32,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
}

/// Per-provider circuit breakers, keyed by registry name.
#[derive(Debug, Default)]
pub(crate) struct Circuits {
    states: Mutex<HashMap<String, CircuitState>>,
}

impl Circuits {
    /// Gate for one judgment call. Errors when the circuit is open (or a
    /// half-open probe is already in flight); otherwise flips to half-open
    /// and admits the caller as the probe when the cooldown has elapsed.
    pub(crate) fn check(&self, name: &str, cooldown: Duration) -> Result<(), TdmError> {
        let mut states = self
            .states
            .lock()
            .expect("circuit mutex must not be poisoned");
        let state = states.entry(name.to_owned()).or_default();
        match state.opened_at {
            None => Ok(()),
            Some(opened_at) if opened_at.elapsed() >= cooldown => {
                if state.probe_in_flight {
                    Err(circuit_open_error())
                } else {
                    state.probe_in_flight = true;
                    tracing::debug!(provider = name, "circuit half-open; admitting probe");
                    Ok(())
                }
            }
            Some(_) => Err(circuit_open_error()),
        }
    }

    /// Records a successful provider outcome: closes the circuit.
    pub(crate) fn record_success(&self, name: &str) {
        let mut states = self
            .states
            .lock()
            .expect("circuit mutex must not be poisoned");
        let state = states.entry(name.to_owned()).or_default();
        state.consecutive_failures = 0;
        state.opened_at = None;
        state.probe_in_flight = false;
    }

    /// Records a Network/Server outcome: increments the consecutive-failure
    /// counter, opening the circuit once it reaches `threshold`. A failed
    /// half-open probe re-opens the circuit for a full cooldown.
    pub(crate) fn record_failure(&self, name: &str, threshold: u32) {
        let mut states = self
            .states
            .lock()
            .expect("circuit mutex must not be poisoned");
        let state = states.entry(name.to_owned()).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.probe_in_flight {
            state.opened_at = Some(Instant::now());
            state.probe_in_flight = false;
        } else if state.consecutive_failures >= threshold.max(1) && state.opened_at.is_none() {
            state.opened_at = Some(Instant::now());
            tracing::warn!(provider = name, "circuit opened after consecutive failures");
        }
    }

    /// Records a non-retryable outcome (Client/Quality/Unsupported): the
    /// consecutive-failure counter is untouched, but an in-flight probe is
    /// considered complete.
    pub(crate) fn record_non_retryable(&self, name: &str) {
        let mut states = self
            .states
            .lock()
            .expect("circuit mutex must not be poisoned");
        if let Some(state) = states.get_mut(name) {
            state.probe_in_flight = false;
        }
    }
}
