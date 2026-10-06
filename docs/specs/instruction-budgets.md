# Host-owned execution budgets

## Contract

`Lua::state_mut()` exposes these embedding APIs on `LuaState`:

- `set_instruction_budget(owner: &str, limit: Option<u64>)`: configure and reset usage. None means metered but unlimited; zero denies execution.
- `instruction_budget(owner) -> Option<InstructionBudget>`: snapshot `{ limit, used }`.
- `reset_instruction_usage(owner) -> LuaResult<()>`: refill at the host's chosen budget-period boundary; unknown owners fail.
- `with_instruction_owner(owner, closure) -> LuaResult<T>`: enter/leave a trusted owner scope; unknown owners fail. Nested host scopes temporarily replace the owner.
- `with_instruction_budget_exemption(closure) -> T`: enter/leave an exemption, including nested owner scopes. Scope state is restored after success, Lua error or unwinding panic. Panic-abort cannot restore a terminated process.

Owner strings may be addon names/taint tags, but **ownership is selected by trusted Rust dispatch, not Lua taint**. Existing taint stamps live in a Lua-visible registry and debug APIs can change stack taint: deriving trusted attribution from either would permit evasion. The host must scope every untrusted entry (and delayed coroutine resumes); merely configuring a limit does not identify its owner. Do not expose arbitrary owner selection, resets, configuration or exemptions to Lua. No globals are added.

Each dispatched Lua instruction is charged before it runs. Rust callback work, compilation, allocations and elapsed time are not measured. Count/line-hook Lua, nested calls, protected calls, securecall and coroutine execution inside the scope are charged to that owner. Auxiliary/skipped bytecode words are not separate dispatches. Usage saturates at u64::MAX; limited usage stops at the limit. Metering never allocates or looks up a name per instruction.

Exhaustion raises a Runtime error naming the owner while the offending frame is still live; saved_pc identifies its rejected instruction. pcall/coroutine.resume cannot permit additional execution inside an exhausted owner scope: the next instruction is also denied. Different host owner scopes have independent counters. Errors do not refund consumption: only a host reset/configuration refills. Owner/exemption state always restores at scope exit, so subsequent unrelated dispatch is unaffected.

INFERRED: this is an instruction allowance, not a native WoW wall-time throttle. Exemption pauses accounting and enforcement, never refills budgets. The host chooses periods, limits, ownership and event policy. The retained 12.0.5 notes say addon throttles no longer apply while processing PLAYER_LOGOUT and ADDONS_UNLOADING. Dispatch those events within exemption scopes; event production belongs to the simulator, not rilua.

## Verification

Unit tests cover exact limit/zero/reset, independent owners, nested exemptions and unwind restoration. `tests/helpers/instruction_budget.rs` covers debug/taint tampering, normal exhaustion, exemption success/error, reset, coroutine/pcall evasion and nested host ownership.

Disabled by default: dispatch checks one optional host owner; no counters, owner lookup, clock reads or hooks run when no owner scope is active. Criterion `vm_execution/control_flow_dispatch` is compared against base a76ffa8 to check unused-feature cost.
