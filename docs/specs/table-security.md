# Opt-in table security core

## Contract

- Embedders explicitly register `settablesecurity`, `secretwrap`, `secretunwrap`, and `issecretvalue`; ordinary Lua states do not publish them.
- `settablesecurity(table, option)` returns no values. Option 0 disallows tainted access; option 1 disallows secret keys. Repeated options accumulate, matching the Forever Cooldown Viewer backing-table sequence.
- Invalid arguments fail explicitly. Option 2 (`SecretWrapContents`) marks shallow contents secret; existing values and future checked writes are stored as wrappers. Immutable/frozen tables reject enabling option 2. Repeated options are idempotent.
- Shared access validation rejects tainted callers, including tainted ancestors, for option 0; option 1 rejects opaque secret keys. Trusted direct table/arena APIs remain below this validator.
- Secret wrappers retain original values through GC without storing payloads in a Lua-visible environment or metatable. Unwrapping preserves original key identity, including independently wrapped copies of the same value.
- Lua-value `wrap_secret` and `unwrap_secret` require an untainted caller. Wrapping an existing wrapper is idempotent. Trusted Rust may mint host-computed booleans, numbers, and strings through `wrap_host_secret_bool(&mut LuaState, bool) -> Val`, `wrap_host_secret_number(&mut LuaState, f64) -> Val`, and `wrap_host_secret_string(&mut LuaState, &str) -> Val`. These typed producers are not Lua entry points, accept no arbitrary Lua value, and preserve caller taint even when called by addon code. Strings are copied into VM-owned storage. Results must be rooted before another GC safe point. These producers neither declassify values nor bypass the existing Lua-value wrapping and unwrapping guards; strings remain opaque. Varargs preserve nils and arity; ordinary unwrapped values pass through unwrapping.
- Flags belong to table objects and disappear on collection. Wrapper payloads are traced during normal marking, finalizer marking, and frozen-graph traversal.

## Evidence and limits

Source: Forever 1.60.1.69913 `Blizzard_APIDocumentationGenerated/FrameScriptDocumentation.lua` and `Blizzard_CooldownViewer/CooldownViewerSecure.lua`.

The API documentation gives option values 0–2 and wrap/unwrap descriptions; the consumer applies options 1 then 0 to its backing table and 0 to its proxy. Error text, opaque-userdata representation, strict argument validation, and rejected tainted operations are simulator policy, not native-conformance claims.

This core supplies validators, not automatic enforcement of every Lua/host operation. Interpreter and standard-library callers must wire the validator separately. Guarded boundaries inspect secret booleans for secure evaluation of `not` and equality, yielding public results; `and`/`or` retain the wrapper. Wrapped nil is guarded for equality before wrapper identity can disclose it. Secure callers may use ordinary Lua indexing on a wrapped table, including its `__index` chain; returned fields are plain Lua values. This shallow table-read rule is native-unverified and does not recursively propagate secrecy. Guarded VM `<` and `<=` comparisons (including Lua's reversed `>` and `>=` lowering), and host API less-than comparisons, inspect secret numbers and compare them with public numbers or other secret numbers. Stack-wide taint denies inspection even when a tainted closure is entered through a secure call. Nonnumeric wrappers remain rejected by ordering; numeric arithmetic and equality stay opaque. This numeric ordering rule is an inferred WoW compatibility contract from the Forever aura-row consumer, not native-verified behavior. This bounded policy does not claim native automatic secrecy propagation. Secret arithmetic, native equality of distinct wrappers, recursive wrapping of table contents, and general secret-value VM semantics are not implemented here. Native metadata for these boolean rules is unknown.

## Lua-facing standard-library boundaries

Raw reads/writes, iteration (including previously captured iterators), unpack, table-library operations, normal/debug metatable access, `secureexecuterange`, and `os.time` date-table reads check the caller before accessing protected tables. Table-library operations recheck after callbacks before further reads or writes. Fallible public table handles and `Lua::table_next` apply the same checks when used by Lua-invoked Rust functions.

Infallible embedding-only `Table::raw_len`, `LuaApi::table_raw_len`, and `LuaApiMut::get_global_val` remain trusted host operations. A host exposing them to Lua must check access first. Direct arena access is likewise trusted; this is not a sandbox against a malicious embedder.

`tests/helpers/table_security_stdlib.rs` covers these stdlib boundaries, secure access, tainted rejection without mutation, secret-key rejection/unwrapping, retained iterators, and Lua-invoked Rust functions using checked handles. These stdlib fixtures use live `debug.setstacktaint`. Separate `tests/helpers/closure_taint.rs` regressions cover closure-object stamps through ordinary calls, protected/nested calls, tail calls, delayed callbacks, and coroutine entry.

## Opaque secret-string formatting

`string.format` copies VM-private secret-string payload bytes for `%s`, ignoring both precision (`%.5s` preserves all of `abcdefgh`) and width (`%12s` and `%-12s` add no padding). Public strings retain ordinary width/precision behavior. Successful formatting returns a VM-owned secret string if any input is secret, including unused arguments; mixed public fields still honor their own formatting. Payloads and result provenance survive GC. Non-string secrets are not decoded by `%s`; secret numeric conversions and `%q` are outside this contract.

The retained WoW 12.0.5 API change states: “String formatting APIs no longer honor field width modifiers for secret string values (e.g. the `"%.5s"` format will no longer truncate a secret string).” Tainted addon calls are permitted as opaque formatting operations, retaining secret output and caller stack taint. That permission and result-provenance policy are explicit inferences from addon-focused notes, not native-verified semantics. Existing public `secretunwrap` and host `unwrap_secret` guards remain unchanged; no decoded Lua callback or public payload accessor is added. `SetFormattedText` consumer storage/display policy is outside this VM fix.

`tests/helpers/secret_string_formatting.rs`, grouped in the existing integration target, covers host-created payloads, full precision, width/alignment, public controls, mixed arguments, unused secret input provenance, GC after releasing the input, and tainted closure calls with rejected input/output unwrapping.

## Secret payload kind and contents

`SecretPayloadKind` enumerates Nil, Boolean, Number, String, Table, Function, Userdata, Thread and LightUserdata. `secret_payload_kind(&LuaState, Val) -> Option<SecretPayloadKind>` returns only the tag of an authentic live wrapper, with no unwrap permission needed. It never returns identity, bytes or contents; public values, ordinary userdata and stale wrapper handles return None. `is_secret_table(&LuaState, Val) -> bool` tests wrapped-table kind or the SecretWrapContents flag. A plain table containing a secret is not a secret table. Embedders can bind issecrettable without granting addon unwrap or changing taint.

Cached FrameScriptDocumentation defines issecrettable as true when the table is itself secret or table flags make accesses produce secrets. INFERRED bounded option-2 policy: eager shallow wrapping of existing values and checked Lua/API writes. Normal/raw reads, captured iterators, unpack, table.remove and table callbacks therefore return wrappers; `__index` results are wrapped across table/function redirect chains (including unflagged proxies into flagged tables). Existing wrappers keep identity. Keys, length and missing nil remain public, deletion still uses public nil. Nested tables become opaque wrappers, not recursively flagged tables; guarded unwrap preserves their identity. Flagging a table never marks aliases to previously read public values secret. Direct arena/Table mutations remain trusted host operations and must preserve this invariant; checked Table handles and LuaApiMut writes do so automatically. No general information-flow or native conformance claim is made.

Unit tag/flag tests and `tests/helpers/secret_kind.rs` cover tainted metadata inspection without unwrap, repeated flags, contents reads/writes, retained iterators, nested table wrappers, raw access, insert/remove/unpack, metamethod results and GC survival.

## Call-context access revocation

`revoke_secret_access(&mut LuaState) -> LuaResult<()>` targets the current Lua frame, or the immediate caller of a Rust binding. Calling with no active caller fails. `can_access_secrets(&LuaState) -> bool` queries the unwrap guard: untainted and no live revoked ancestor or saved coroutine resumer. Revocation does not alter taint or `issecure`; Lua debug taint changes and `securecall` cannot undo it. Boolean/nil inspection, numeric ordering, and wrapped-table reads use the same unwrap guard. Public nonsecret values remain accessible.

Cached FrameScriptDocumentation says dropsecretaccess "Removes the ability for the immediate calling function to access secret values." INFERRED lifetime: that frame and descendants lose access until the frame exits (normal return/error). Tail-call replacements keep denial; suspended coroutines keep it across yield/resume. A coroutine entered by a revoked context cannot bypass denial while its resumer remains live. After the revoked function ends, its caller regains its previous access. No Lua global is automatically added: the embedder installs its own binding.

Unit frame tests and `tests/helpers/secret_access.rs` exercise normal/error restoration, taint independence, descendant/tail calls, securecall, and coroutine entry/yield.

## Trusted opaque string transforms

`transform_host_secret_string(&mut LuaState, Val, impl FnOnce(&[u8]) -> LuaResult<Vec<u8>>) -> LuaResult<Val>` accepts only an authentic secret string. Trusted Rust receives bytes, never Lua. Output is always a new VM-owned secret string, even for empty/unchanged output; input and caller taint remain unchanged. NUL and invalid UTF-8 are preserved. Host errors propagate without modifying input. Results need rooting before a GC safe point. Hosts must not leak plaintext through side effects or errors; this is not a sandbox against trusted Rust. INFERRED: always-secret output and strict rejection of public/non-string values are the narrow contract needed for Ambiguate and ReplaceIconAndGroupExpressions; native transformation rules remain simulator responsibilities.

Unit binary/error tests and `tests/helpers/host_secret_transform.rs` cover tainted shortening, rejected Lua unwrap, retained taint, NUL/UTF-8 and GC survival.

## Closure stamp propagation

Call entry reads the existing `debug.setobjecttaint` registry stamp and applies it to the new Lua or Rust call frame. Lua closures created while any active call frame is tainted receive that effective taint stamp, including when trusted code is called by a tainted caller; a secure call suspends that inherited taint for closures it creates. Tail-call frame reuse preserves callee stamps and an insecure caller's taint. Secure calls temporarily clear the caller chain and restore it on return or error, without removing the callee's own stamp. Clearing an object's stamp with nil restores untainted entry from an untainted caller.

Closure stamps are retained through GC marking and published root traversal, while weak references prevent collection or slot reuse from transferring a stamp. This is a bounded simulator policy, not a claim of complete native secret-VM or information-flow conformance.

## Proof scope

`tests/helpers/table_security.rs` exercises opt-in publication, arguments/arity, accumulated restrictions, caller taint, opaque key rejection, unwrap identity, and GC reachability/collection. It belongs to the existing integration binary, not a separate Cargo target.
