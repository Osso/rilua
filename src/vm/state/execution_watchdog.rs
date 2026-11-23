//! Trusted owner scopes: no ownership state lives in Lua tables.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::{Duration, Instant};

use super::LuaState;
use crate::{LuaResult, runtime_error};

pub const DEFAULT_WATCHDOG_DISPATCH_INTERVAL: u32 = 10_001;

#[derive(Default)]
pub(super) struct ExecutionWatchdogs {
    owners: Vec<String>,
    pub(super) active: Option<usize>,
    watchdog: Option<ExecutionWatchdog>,
    throttle: Option<ExecutionThrottle>,
    buckets: HashMap<String, ScriptBucket>,
}

struct ExecutionWatchdog {
    deadline: Instant,
    dispatch_interval: u32,
    dispatches_remaining: u32,
}

struct ExecutionThrottle {
    owner: String,
    rate_ms_per_second: u64,
    burst_ms: u64,
    dispatch_interval: u32,
    dispatches_remaining: u32,
    last_charge: Instant,
}

struct ScriptBucket {
    remaining_ms: f64,
    last_refill: Instant,
}

impl LuaState {
    /// Run a host-owned Lua invocation under an elapsed-time watchdog.
    /// The watchdog only checks VM dispatch steps while a trusted owner is active.
    pub fn with_execution_watchdog<T>(
        &mut self,
        timeout: Duration,
        dispatch_interval: u32,
        operation: impl FnOnce(&mut Self) -> LuaResult<T>,
    ) -> LuaResult<T> {
        if dispatch_interval == 0 {
            return Err(runtime_error("watchdog dispatch interval must be positive"));
        }
        let previous = self.instruction_budgets.watchdog.take();
        if self.instruction_budgets.active.is_some() {
            self.instruction_budgets.watchdog = Some(ExecutionWatchdog {
                deadline: Instant::now() + timeout,
                dispatch_interval,
                dispatches_remaining: dispatch_interval,
            });
        }
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        self.instruction_budgets.watchdog = previous;
        finish_scope(result)
    }

    /// Run Lua execution under an owner's time-based script bucket.
    pub fn with_script_bucket_throttle<T>(
        &mut self,
        owner: &str,
        rate_ms_per_second: u64,
        burst_ms: u64,
        dispatch_interval: u32,
        operation: impl FnOnce(&mut Self) -> LuaResult<T>,
    ) -> LuaResult<T> {
        if dispatch_interval == 0 {
            return Err(runtime_error("throttle dispatch interval must be positive"));
        }
        if !self.instruction_budgets.buckets.contains_key(owner) {
            let now = Instant::now();
            self.instruction_budgets.buckets.insert(
                owner.to_owned(),
                ScriptBucket {
                    remaining_ms: burst_ms as f64,
                    last_refill: now,
                },
            );
        }
        let previous = self.instruction_budgets.throttle.take();
        if self.instruction_budgets.active.is_some() {
            self.instruction_budgets.throttle = Some(ExecutionThrottle {
                owner: owner.to_owned(),
                rate_ms_per_second,
                burst_ms,
                dispatch_interval,
                dispatches_remaining: dispatch_interval,
                last_charge: Instant::now(),
            });
        }
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        let charge_result = if result.is_ok() {
            self.charge_script_bucket_throttle()
        } else {
            Ok(())
        };
        self.instruction_budgets.throttle = previous;
        match result {
            Ok(operation_result) => {
                charge_result?;
                operation_result
            }
            Err(panic) => resume_unwind(panic),
        }
    }

    /// Run Lua execution under a trusted owner scope; timing is configured separately.
    pub fn with_instruction_owner<T>(
        &mut self,
        owner: &str,
        operation: impl FnOnce(&mut Self) -> LuaResult<T>,
    ) -> LuaResult<T> {
        let index = self.find_instruction_owner(owner).unwrap_or_else(|| {
            self.instruction_budgets.owners.push(owner.to_owned());
            self.instruction_budgets.owners.len() - 1
        });
        let previous = self.instruction_budgets.active;
        self.instruction_budgets.active = Some(index);
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        self.instruction_budgets.active = previous;
        finish_scope(result)
    }

    #[inline]
    pub(crate) fn has_execution_watchdog(&self) -> bool {
        self.instruction_budgets.watchdog.is_some() || self.instruction_budgets.throttle.is_some()
    }

    /// Poll active watchdog and bucket limits after each VM dispatch step.
    pub(crate) fn poll_execution_watchdog(&mut self) -> LuaResult<()> {
        self.poll_deadline_watchdog()?;
        self.poll_script_bucket_throttle()
    }

    fn poll_deadline_watchdog(&mut self) -> LuaResult<()> {
        let Some(watchdog) = self.instruction_budgets.watchdog.as_mut() else {
            return Ok(());
        };
        watchdog.dispatches_remaining -= 1;
        if watchdog.dispatches_remaining != 0 {
            return Ok(());
        }
        watchdog.dispatches_remaining = watchdog.dispatch_interval;
        if Instant::now() >= watchdog.deadline {
            return Err(runtime_error("script ran too long"));
        }
        Ok(())
    }

    fn poll_script_bucket_throttle(&mut self) -> LuaResult<()> {
        let Some(throttle) = self.instruction_budgets.throttle.as_mut() else {
            return Ok(());
        };
        throttle.dispatches_remaining -= 1;
        if throttle.dispatches_remaining != 0 {
            return Ok(());
        }
        throttle.dispatches_remaining = throttle.dispatch_interval;
        self.charge_script_bucket_throttle()
    }

    fn charge_script_bucket_throttle(&mut self) -> LuaResult<()> {
        let Some(throttle) = self.instruction_budgets.throttle.as_mut() else {
            return Ok(());
        };
        let now = Instant::now();
        let bucket = self
            .instruction_budgets
            .buckets
            .get_mut(&throttle.owner)
            .unwrap();
        refill_bucket(bucket, throttle.rate_ms_per_second, throttle.burst_ms, now);
        let elapsed_ms = now.duration_since(throttle.last_charge).as_secs_f64() * 1000.0;
        throttle.last_charge = now;
        if elapsed_ms >= bucket.remaining_ms {
            bucket.remaining_ms = 0.0;
            return Err(runtime_error(format!(
                "insecure scripts exceeded execution limit for addon {}",
                throttle.owner
            )));
        }
        bucket.remaining_ms -= elapsed_ms;
        Ok(())
    }

    fn find_instruction_owner(&self, owner: &str) -> Option<usize> {
        self.instruction_budgets
            .owners
            .iter()
            .position(|name| name == owner)
    }
}

fn refill_bucket(bucket: &mut ScriptBucket, rate_ms_per_second: u64, burst_ms: u64, now: Instant) {
    if bucket.remaining_ms >= burst_ms as f64 {
        bucket.last_refill = now;
        return;
    }
    let elapsed_seconds = now.duration_since(bucket.last_refill).as_secs();
    if elapsed_seconds == 0 {
        return;
    }
    let credit = elapsed_seconds as f64 * rate_ms_per_second as f64;
    bucket.remaining_ms = (bucket.remaining_ms + credit).min(burst_ms as f64);
    bucket.last_refill = now;
}

fn finish_scope<T>(result: std::thread::Result<T>) -> T {
    match result {
        Ok(value) => value,
        Err(panic) => resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_throttle_charges_the_final_invocation_slice() {
        let mut state = LuaState::new();
        let result = state.with_instruction_owner("ExampleAddon", |state| {
            state.with_script_bucket_throttle("ExampleAddon", 0, 0, 1, |_| Ok(()))
        });
        assert_eq!(
            result.unwrap_err().to_string(),
            "insecure scripts exceeded execution limit for addon ExampleAddon"
        );
    }

    #[test]
    fn watchdog_checks_deadline_at_configured_dispatch_interval() -> LuaResult<()> {
        let mut state = LuaState::new();
        state.with_instruction_owner("Addon", |state| {
            state.with_execution_watchdog(Duration::from_secs(60), 2, |state| {
                state.poll_execution_watchdog()?;
                assert!(state.has_execution_watchdog());
                state.poll_execution_watchdog()?;
                Ok(())
            })?;
            state.with_execution_watchdog(Duration::ZERO, 2, |state| {
                state.poll_execution_watchdog()?;
                assert!(
                    state
                        .poll_execution_watchdog()
                        .is_err_and(|error| { error.to_string().contains("script ran too long") })
                );
                Ok(())
            })
        })?;
        assert!(!state.has_execution_watchdog());
        Ok(())
    }
}
