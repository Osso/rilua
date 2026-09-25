# Opt-in table security core

## Contract

- Embedders explicitly register `settablesecurity`, `secretwrap`, `secretunwrap`, and `issecretvalue`; ordinary Lua states do not publish them.
- `settablesecurity(table, option)` returns no values. Option 0 disallows tainted access; option 1 disallows secret keys. Repeated options accumulate, matching the Forever Cooldown Viewer backing-table sequence.
- Invalid arguments fail explicitly. Option 2 (`SecretWrapContents`) fails as unsupported, without changing existing restrictions.
- Shared access validation rejects tainted callers, including tainted ancestors, for option 0; option 1 rejects opaque secret keys. Trusted direct table/arena APIs remain below this validator.
- Secret wrappers retain original values through GC without storing payloads in a Lua-visible environment or metatable. Unwrapping preserves original key identity, including independently wrapped copies of the same value.
- Lua-value `wrap_secret` and `unwrap_secret` require an untainted caller. Wrapping an existing wrapper is idempotent. Trusted Rust may mint only a host-computed boolean through `wrap_host_secret_bool`; it is not a Lua entry point, accepts no Lua value, and preserves caller taint. Varargs preserve nils and arity; ordinary unwrapped values pass through unwrapping.
- Flags belong to table objects and disappear on collection. Wrapper payloads are traced during normal marking, finalizer marking, and frozen-graph traversal.

## Evidence and limits

Source: Forever 1.60.1.69913 `Blizzard_APIDocumentationGenerated/FrameScriptDocumentation.lua` and `Blizzard_CooldownViewer/CooldownViewerSecure.lua`.

The API documentation gives option values 0–2 and wrap/unwrap descriptions; the consumer applies options 1 then 0 to its backing table and 0 to its proxy. Error text, opaque-userdata representation, strict argument validation, and rejected tainted operations are simulator policy, not native-conformance claims.

This core supplies validators, not automatic enforcement of every Lua/host operation. Interpreter and standard-library callers must wire the validator separately. Guarded boundaries inspect secret booleans for secure evaluation of `not` and equality, yielding public results; `and`/`or` retain the wrapper. Wrapped nil is guarded for equality before wrapper identity can disclose it. Secure callers may use ordinary Lua indexing on a wrapped table, including its `__index` chain; returned fields are plain Lua values. This shallow table-read rule is native-unverified and does not recursively propagate secrecy. Guarded VM `<` and `<=` comparisons (including Lua's reversed `>` and `>=` lowering), and host API less-than comparisons, inspect secret numbers and compare them with public numbers or other secret numbers. Stack-wide taint denies inspection even when a tainted closure is entered through a secure call. Nonnumeric wrappers remain rejected by ordering; numeric arithmetic and equality stay opaque. This numeric ordering rule is an inferred WoW compatibility contract from the Forever aura-row consumer, not native-verified behavior. This bounded policy does not claim native automatic secrecy propagation. Secret arithmetic, native equality of distinct wrappers, automatic wrapping of table contents, and general secret-value VM semantics are not implemented here. Native metadata for these boolean rules is unknown.

## Lua-facing standard-library boundaries

Raw reads/writes, iteration (including previously captured iterators), unpack, table-library operations, normal/debug metatable access, `secureexecuterange`, and `os.time` date-table reads check the caller before accessing protected tables. Table-library operations recheck after callbacks before further reads or writes. Fallible public table handles and `Lua::table_next` apply the same checks when used by Lua-invoked Rust functions.

Infallible embedding-only `Table::raw_len`, `LuaApi::table_raw_len`, and `LuaApiMut::get_global_val` remain trusted host operations. A host exposing them to Lua must check access first. Direct arena access is likewise trusted; this is not a sandbox against a malicious embedder.

`tests/helpers/table_security_stdlib.rs` covers these stdlib boundaries, secure access, tainted rejection without mutation, secret-key rejection/unwrapping, retained iterators, and Lua-invoked Rust functions using checked handles. These stdlib fixtures use live `debug.setstacktaint`. Separate `tests/helpers/closure_taint.rs` regressions cover closure-object stamps through ordinary calls, protected/nested calls, tail calls, delayed callbacks, and coroutine entry.

## Closure stamp propagation

Call entry reads the existing `debug.setobjecttaint` registry stamp and applies it to the new Lua or Rust call frame. Lua closures created while any active call frame is tainted receive that effective taint stamp, including when trusted code is called by a tainted caller; a secure call suspends that inherited taint for closures it creates. Tail-call frame reuse preserves callee stamps and an insecure caller's taint. Secure calls temporarily clear the caller chain and restore it on return or error, without removing the callee's own stamp. Clearing an object's stamp with nil restores untainted entry from an untainted caller.

Closure stamps are retained through GC marking and published root traversal, while weak references prevent collection or slot reuse from transferring a stamp. This is a bounded simulator policy, not a claim of complete native secret-VM or information-flow conformance.

## Proof scope

`tests/helpers/table_security.rs` exercises opt-in publication, arguments/arity, accumulated restrictions, caller taint, opaque key rejection, unwrap identity, and GC reachability/collection. It belongs to the existing integration binary, not a separate Cargo target.
