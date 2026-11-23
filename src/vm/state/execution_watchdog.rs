//! Trusted owner scopes: no ownership state lives in Lua tables.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::time::{Duration, Instant};

use super::LuaState;
use crate::{LuaResult, runtime_error};

pub const DEFAULT_WATCHDOG_DISPATCH_INTERVAL: u32 = 10_001;

#[derive(Default)]
pub(super) struct ExecutionWatchdogs {
    owners: Vec<String>,
    pub(super) active: Option<usize>,
    exempt: bool,
    watchdog: Option<ExecutionWatchdog>,
}

struct ExecutionWatchdog {
    deadline: Instant,
    dispatch_interval: u32,
    dispatches_remaining: u32,
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
        if self.instruction_budgets.active.is_some() && !self.instruction_budgets.exempt {
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

    /// Run host callbacks for the configured dispatch owner without timing them.
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
        self.instruction_budgets.active = if self.instruction_budgets.exempt {
            None
        } else {
            Some(index)
        };
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        self.instruction_budgets.active = previous;
        finish_scope(result)
    }

    /// Exclude nested owner callbacks and coroutine dispatch from host timing.
    pub fn with_watchdog_exemption<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> T {
        let previous_owner = self.instruction_budgets.active.take();
        let previous_exempt = self.instruction_budgets.exempt;
        let previous_watchdog = self.instruction_budgets.watchdog.take();
        self.instruction_budgets.exempt = true;
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        self.instruction_budgets.exempt = previous_exempt;
        self.instruction_budgets.active = previous_owner;
        self.instruction_budgets.watchdog = previous_watchdog;
        finish_scope(result)
    }

    #[inline]
    pub(crate) fn has_execution_watchdog(&self) -> bool {
        self.instruction_budgets.watchdog.is_some()
    }

    /// Poll the active deadline after each VM dispatch step.
    pub(crate) fn poll_execution_watchdog(&mut self) -> LuaResult<()> {
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

    fn find_instruction_owner(&self, owner: &str) -> Option<usize> {
        self.instruction_budgets
            .owners
            .iter()
            .position(|name| name == owner)
    }
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
