//! Trusted owner scopes: no ownership or exemption state lives in Lua tables.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use super::LuaState;
use crate::{LuaResult, runtime_error};

/// Snapshot of an owner's cumulative instruction usage since host reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstructionBudget {
    /// None meters without enforcing a limit; zero denies the first instruction.
    pub limit: Option<u64>,
    /// Executed dispatch instructions (not elapsed time or native Rust work).
    pub used: u64,
}

#[derive(Default)]
pub(super) struct InstructionBudgets {
    owners: Vec<(String, InstructionBudget)>,
    pub(super) active: Option<usize>,
    exempt: bool,
}

impl LuaState {
    /// Configure an addon/owner budget, resetting its usage. Host only: owner
    /// strings may match taint tags, but Lua-editable taint never selects owners.
    pub fn set_instruction_budget(&mut self, owner: &str, limit: Option<u64>) {
        let budget = InstructionBudget { limit, used: 0 };
        if let Some(index) = self.find_instruction_owner(owner) {
            self.instruction_budgets.owners[index].1 = budget;
        } else {
            self.instruction_budgets
                .owners
                .push((owner.to_owned(), budget));
        }
    }

    /// Query cumulative usage without modifying it.
    pub fn instruction_budget(&self, owner: &str) -> Option<InstructionBudget> {
        self.find_instruction_owner(owner)
            .map(|index| self.instruction_budgets.owners[index].1)
    }

    /// Start a host-defined new budget period. Errors do not refund usage.
    pub fn reset_instruction_usage(&mut self, owner: &str) -> LuaResult<()> {
        let index = self
            .find_instruction_owner(owner)
            .ok_or_else(|| runtime_error("instruction owner is not configured"))?;
        self.instruction_budgets.owners[index].1.used = 0;
        Ok(())
    }

    /// Charge all Lua execution (including debug hooks and resumed coroutines)
    /// to a configured trusted owner. Nested host owner scopes replace it.
    /// Restores the previous scope after success, Lua errors, or Rust unwinding.
    /// Hosts must wrap every untrusted dispatch, and must not expose owner
    /// selection or this API through attacker-controlled Lua arguments.
    pub fn with_instruction_owner<T>(
        &mut self,
        owner: &str,
        operation: impl FnOnce(&mut Self) -> LuaResult<T>,
    ) -> LuaResult<T> {
        let index = self
            .find_instruction_owner(owner)
            .ok_or_else(|| runtime_error("instruction owner is not configured"))?;
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

    /// Enter a host-controlled exemption for this dynamic scope, including
    /// nested owner scopes and coroutine dispatch. No usage is charged.
    /// Leaving the closure restores enforcement even after errors or panics.
    /// INFERRED: exemption pauses usage, never refills an exhausted budget.
    pub fn with_instruction_budget_exemption<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let previous_owner = self.instruction_budgets.active.take();
        let previous_exempt = self.instruction_budgets.exempt;
        self.instruction_budgets.exempt = true;
        let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
        self.instruction_budgets.exempt = previous_exempt;
        self.instruction_budgets.active = previous_owner;
        finish_scope(result)
    }

    fn find_instruction_owner(&self, owner: &str) -> Option<usize> {
        self.instruction_budgets
            .owners
            .iter()
            .position(|(name, _)| name == owner)
    }

    #[inline]
    pub(crate) fn instruction_meter_owner(&self) -> Option<usize> {
        self.instruction_budgets.active
    }

    /// Called before dispatch; exhaustion leaves the offending frame live.
    pub(crate) fn charge_instruction(&mut self, index: usize) -> LuaResult<()> {
        let (owner, budget) = &mut self.instruction_budgets.owners[index];
        if budget.limit.is_some_and(|limit| budget.used >= limit) {
            return Err(runtime_error(format!(
                "instruction budget exhausted for owner '{owner}'"
            )));
        }
        budget.used = budget.used.saturating_add(1);
        Ok(())
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

    fn charge_dispatch(state: &mut LuaState) -> LuaResult<()> {
        if let Some(owner) = state.instruction_meter_owner() {
            state.charge_instruction(owner)?;
        }
        Ok(())
    }

    #[test]
    fn instruction_budget_exact_limit_reset_and_nested_restoration() -> LuaResult<()> {
        let mut state = LuaState::new();
        state.set_instruction_budget("A", Some(1));
        state.set_instruction_budget("B", Some(0));
        state.with_instruction_owner("A", |state| {
            charge_dispatch(state)?;
            assert!(state.with_instruction_owner("B", charge_dispatch).is_err());
            state.with_instruction_budget_exemption(|state| {
                state.with_instruction_budget_exemption(charge_dispatch)?;
                charge_dispatch(state)
            })?;
            assert!(charge_dispatch(state).is_err());
            Ok(())
        })?;
        charge_dispatch(&mut state)?;
        assert_eq!(
            state.instruction_budget("A"),
            Some(InstructionBudget {
                limit: Some(1),
                used: 1
            })
        );
        state.reset_instruction_usage("A")?;
        assert_eq!(state.instruction_budget("A").map(|b| b.used), Some(0));
        Ok(())
    }

    #[test]
    fn instruction_budget_scopes_restore_after_host_unwind() -> LuaResult<()> {
        let mut state = LuaState::new();
        state.set_instruction_budget("A", Some(1));
        let panic = catch_unwind(AssertUnwindSafe(|| {
            state.with_instruction_owner("A", |state| -> LuaResult<()> {
                state.with_instruction_budget_exemption(|_| {
                    std::panic::resume_unwind(Box::new("test unwind"))
                })
            })
        }));
        assert!(panic.is_err());
        charge_dispatch(&mut state)?;
        assert_eq!(
            state.instruction_budget("A").map(|budget| budget.used),
            Some(0)
        );
        state.with_instruction_owner("A", charge_dispatch)?;
        assert_eq!(
            state.instruction_budget("A").map(|budget| budget.used),
            Some(1)
        );
        assert!(state.with_instruction_owner("A", charge_dispatch).is_err());
        Ok(())
    }
}
