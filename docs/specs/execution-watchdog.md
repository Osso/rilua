# Host-owned execution watchdogs

## Contract

`Lua::state_mut()` exposes these host APIs on `LuaState`:

- `with_instruction_owner(owner, closure) -> LuaResult<T>`: enter and leave a trusted owner scope. Nested scopes temporarily replace the active owner. The host selects ownership; Lua taint does not.
- `with_execution_watchdog(timeout, dispatch_interval, closure) -> LuaResult<T>`: start a per-invocation deadline and poll it at the requested dispatch interval. A zero interval is rejected. The host controls the invocation boundary and timeout.


`DEFAULT_WATCHDOG_DISPATCH_INTERVAL` is 10,001 dispatch steps. At each poll, the VM compares the current monotonic time with the invocation deadline. An expired deadline raises `script ran too long`. The deadline is set when `with_execution_watchdog` starts, not at the last poll. Rust callback work between VM dispatch steps is included in elapsed time, but cannot be interrupted until the next poll. Compilation is outside a call unless the host wraps it explicitly.

The watchdog runs only while the host has entered an owner scope. This keeps it away from secure/unowned execution. Owner names remain host-controlled; do not expose arbitrary owner selection or watchdog configuration to Lua. Nested synchronous Lua calls and resumed coroutines use the active invocation watchdog. The watchdog is restored at the host scope boundary.

The prior cumulative instruction quota, per-owner usage snapshots, reset API and `instruction budget exhausted` error are removed. This watchdog is not the simulator's separate bucket throttle.

## Verification

`tests/helpers/execution_watchdog.rs` verifies an under-threshold owner invocation, an overrun with the exact error, recovery after the error, nested owner scopes and coroutine dispatch. The unit test in `src/vm/state/execution_watchdog.rs` verifies the configured poll interval and deadline behavior.

The VM specializes execution at invocation entry. The unmetered loop has no per-dispatch watchdog check; the watchdog loop checks the counter at each dispatch and reads the clock only when the configured poll interval expires.
