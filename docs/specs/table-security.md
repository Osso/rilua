# Opt-in table security core

## Contract

- Embedders explicitly register `settablesecurity`, `secretwrap`, `secretunwrap`, and `issecretvalue`; ordinary Lua states do not publish them.
- `settablesecurity(table, option)` returns no values. Option 0 disallows tainted access; option 1 disallows secret keys. Repeated options accumulate, matching the Forever Cooldown Viewer backing-table sequence.
- Invalid arguments fail explicitly. Option 2 (`SecretWrapContents`) fails as unsupported, without changing existing restrictions.
- Shared access validation rejects tainted callers, including tainted ancestors, for option 0; option 1 rejects opaque secret keys. Trusted direct table/arena APIs remain below this validator.
- Secret wrappers retain original values through GC without storing payloads in a Lua-visible environment or metatable. Unwrapping preserves original key identity, including independently wrapped copies of the same value.
- Wrapping and unwrapping secret payloads require an untainted caller. Wrapping an existing wrapper is idempotent. Varargs preserve nils and arity; ordinary unwrapped values pass through unwrapping.
- Flags belong to table objects and disappear on collection. Wrapper payloads are traced during normal marking, finalizer marking, and frozen-graph traversal.

## Evidence and limits

Source: Forever 1.60.1.69913 `Blizzard_APIDocumentationGenerated/FrameScriptDocumentation.lua` and `Blizzard_CooldownViewer/CooldownViewerSecure.lua`.

The API documentation gives option values 0–2 and wrap/unwrap descriptions; the consumer applies options 1 then 0 to its backing table and 0 to its proxy. Error text, opaque-userdata representation, strict argument validation, and rejected tainted operations are simulator policy, not native-conformance claims.

This core supplies validators, not automatic enforcement of every Lua/host operation. Interpreter and standard-library callers must wire the validator separately. Full secret arithmetic/type behavior, native equality of distinct wrappers, automatic wrapping of table contents, and general secret-value VM semantics are not implemented here.

## Lua-facing standard-library boundaries

Raw reads/writes, iteration (including previously captured iterators), unpack, table-library operations, normal/debug metatable access, `secureexecuterange`, and `os.time` date-table reads check the caller before accessing protected tables. Table-library operations recheck after callbacks before further reads or writes. Fallible public table handles and `Lua::table_next` apply the same checks when used by Lua-invoked Rust functions.

Infallible embedding-only `Table::raw_len`, `LuaApi::table_raw_len`, and `LuaApiMut::get_global_val` remain trusted host operations. A host exposing them to Lua must check access first. Direct arena access is likewise trusted; this is not a sandbox against a malicious embedder.

`tests/helpers/table_security_stdlib.rs` covers these stdlib boundaries, secure access, tainted rejection without mutation, secret-key rejection/unwrapping, retained iterators, and Lua-invoked Rust functions using checked handles. Taint fixtures use live `debug.setstacktaint`; closure-object stamping through `pcall` is not established by this proof.

## Proof scope

`tests/helpers/table_security.rs` exercises opt-in publication, arguments/arity, accumulated restrictions, caller taint, opaque key rejection, unwrap identity, and GC reachability/collection. It belongs to the existing integration binary, not a separate Cargo target.
