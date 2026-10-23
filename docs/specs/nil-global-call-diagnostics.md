# Configurable nil-global call diagnostics

## Contract

`Lua::state_mut().set_named_nil_global_calls(enabled)` configures nil-global call diagnostics for that VM state. It installs no Lua API and has no application/profile concepts.

- [ ] New states default to disabled: a nil global call reports `attempt to call a nil value`.
- [ ] Enabled states report `attempt to call global 'X' (a nil value)` when the existing debug-name resolver recovers global `X`.
- [ ] An inner `pcall` returns exactly two values: false and the named error string. Location prefixes remain ordinary VM diagnostics.
- [ ] Disabling restores generic nil-global wording. The state setting is shared across coroutine execution.
- [ ] Local/field/upvalue/method, non-nil call errors, and calls without a recoverable global name retain existing behavior. This switch changes diagnostics only, not execution or error propagation.

## Verification

`src/lib.rs` retains `global_nil_call_error_matches_retail` unchanged for the default and adds `named_nil_global_calls_reports_direct_call_name` and `named_nil_global_calls_preserves_inner_pcall_two_results` for enabled mode. `src/vm/state.rs` owns the setting; `src/vm/execute.rs` applies it at the existing type-error boundary.

Edit-only preparation: tests are authored, not executed; no RED/GREEN or native-client measurement claimed. Toggle/coroutine and other-error preservation are contract boundaries without new dedicated regression coverage here.
