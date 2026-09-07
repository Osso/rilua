# Lua environment value transfers

Embedders may set `LuaState.environment_transfer_hook` to convert direct arguments and results when a Lua closure crosses between distinct environment tables. The callback receives the source environment, target environment, and the stack range containing values to convert. It may replace those values and reenter Lua, but must preserve the active call frames and stack top. Reentrant calls do not recursively invoke the callback; callback errors propagate to the normal protected-call boundary.

The hook is disabled by default. Calls within the same environment are unchanged. Rust/native functions do not themselves trigger conversion, preserving explicit native return contracts such as an embedding application's raw object-table accessor. Lua callbacks reached through native functions use the nearest Lua caller's environment. Tail calls and fixed/variable arguments/results use the same conversion boundary. `LuaState::poscall` now returns `LuaResult<bool>` so return conversion errors cannot be discarded.

This facility does not recursively rewrite table contents, identify application-specific object types, or establish a security policy. Those decisions belong to the embedder. Coroutine yield/resume transfers are not covered by this facility; no cross-coroutine conversion guarantee is made.

Behavioral tests live in `src/vm/state/environment_transfer.rs`: fixed arguments and varargs, multiple/nil results, local outbound tail returns, native raw returns, native protected-call callbacks, host reentry, errors, and the disabled/same-environment cases.
