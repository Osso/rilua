//! Criterion benchmarks for rilua interpreter hot paths.
//!
//! Run with: `cargo bench`

#![allow(clippy::expect_used)]

use std::fmt::Write as _;

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, black_box, criterion_group, criterion_main};
use rilua::compiler::codegen;
use rilua::{Function, Lua, LuaApiMut, StdLib, Val};

fn load_returned_function(lua: &mut Lua, source: &str, name: &str) -> Function {
    let chunk = lua
        .load_bytes(source.as_bytes(), name)
        .expect("load failed");
    let results = lua
        .call_function(&chunk, &[])
        .expect("bootstrap call failed");
    match results.as_slice() {
        [Val::Function(func)] => Function::from_gc_ref(*func),
        other => unreachable!("expected returned function, got {other:?}"),
    }
}

fn append_large_compile_worker(source: &mut String, func_idx: usize) {
    writeln!(source, "local function worker_{func_idx}(seed)").expect("write failed");
    for local_idx in 0..24 {
        let value = func_idx * 100 + local_idx;
        writeln!(source, "  local v_{func_idx}_{local_idx} = seed + {value}")
            .expect("write failed");
    }
    source.push_str("  local sum = 0\n");
    source.push_str("  for i = 1, 40 do\n");
    for local_idx in 0..24 {
        writeln!(source, "    sum = sum + v_{func_idx}_{local_idx} + i").expect("write failed");
    }
    source.push_str("  end\n");
    source.push_str("  return sum\n");
    source.push_str("end\n");
    writeln!(
        source,
        "total = total + worker_{func_idx}({})",
        func_idx + 1
    )
    .expect("write failed");
}

fn build_large_compile_chunk() -> String {
    let mut source = String::from("local total = 0\n");

    for func_idx in 0..80 {
        append_large_compile_worker(&mut source, func_idx);
    }

    source.push_str("return total\n");
    source
}

fn build_large_execution_chunk() -> String {
    let mut source = String::from("return function()\n");
    source.push_str("  local total = 0\n");

    for local_idx in 0..96 {
        writeln!(&mut source, "  local slot_{local_idx} = {}", local_idx + 1)
            .expect("write failed");
    }

    source.push_str("  for outer = 1, 160 do\n");
    source.push_str("    local row = outer\n");
    for local_idx in 0..96 {
        writeln!(
            &mut source,
            "    row = row + slot_{local_idx} + ((outer + {}) % 7)",
            local_idx + 1
        )
        .expect("write failed");
    }
    source.push_str("    total = total + row\n");
    source.push_str("  end\n");
    source.push_str("  return total\n");
    source.push_str("end\n");
    source
}

fn register_compile_verybig_chunk(group: &mut BenchmarkGroup<'_, WallTime>) {
    let large_src = build_large_compile_chunk();
    group.bench_function("compile_verybig_chunk", |b| {
        b.iter(|| {
            let proto = codegen::compile(black_box(large_src.as_bytes()), "bench");
            black_box(proto).expect("compile failed");
        });
    });
}

fn register_control_flow_dispatch(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("control_flow_dispatch", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            return function()
                local acc = 0
                for outer = 1, 80 do
                    local inner = 60
                    while inner > 0 do
                        if inner % 15 == 0 then
                            acc = acc + outer - inner
                        elseif inner % 5 == 0 then
                            acc = acc + inner
                        elseif inner % 3 == 0 then
                            acc = acc + outer
                        else
                            acc = acc + outer + inner
                        end
                        inner = inner - 1
                    end
                end

                local tail = 1
                repeat
                    acc = acc + tail
                    tail = tail + 1
                until tail > 120

                return acc
            end
            ",
            "control_flow_dispatch",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("control-flow bench failed");
            black_box(results);
        });
    });
}

fn register_verybig_loaded_chunk(group: &mut BenchmarkGroup<'_, WallTime>) {
    let large_execution_src = build_large_execution_chunk();
    group.bench_function("verybig_loaded_chunk", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        let bench = load_returned_function(&mut lua, &large_execution_src, "verybig_loaded_chunk");
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("large execution bench failed");
            black_box(results);
        });
    });
}

fn register_debug_metadata_roundtrip(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("metadata_roundtrip_100", |b| {
        let mut lua =
            Lua::new_with(StdLib::BASE | StdLib::DEBUG | StdLib::STRING).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r#"
            local function inspect(seed)
                local local_seed = seed
                local bias = seed + 1
                local function inner(offset)
                    return local_seed + bias + offset
                end

                local function probe(delta)
                    local info = debug.getinfo(inner, "SufLn")
                    local local_name, local_val = debug.getlocal(1, 1)
                    local up_name, up_val = debug.getupvalue(inner, 1)
                    local trace = debug.traceback("", 2)
                    return info.currentline or 0, local_name, local_val, up_name, up_val, #trace
                end

                return probe(seed)
            end

            return function()
                local total = 0
                for i = 1, 100 do
                    local currentline, local_name, local_val, up_name, up_val, trace_len =
                        inspect(i)
                    total = total + currentline + local_val + up_val + trace_len
                    if local_name == "delta" then
                        total = total + 1
                    end
                    if up_name == "local_seed" then
                        total = total + 1
                    end
                end
                return total
            end
            "#,
            "debug_metadata_roundtrip",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("debug bench failed");
            black_box(results);
        });
    });
}

fn register_next_pairs_mixed(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("next_pairs_mixed_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r#"
            local t = {}
            for i = 1, 1000 do
                t[i] = i
                t["k" .. i] = i * 2
            end

            return function()
                local total = 0
                for _ = 1, 8 do
                    for key, value in pairs(t) do
                        if type(key) == "number" then
                            total = total + value
                        else
                            total = total + value + #key
                        end
                    end

                    local key = nil
                    while true do
                        local next_key, value = next(t, key)
                        if next_key == nil then
                            break
                        end
                        if type(next_key) == "number" then
                            total = total + value
                        else
                            total = total + value + #next_key
                        end
                        key = next_key
                    end
                end
                return total
            end
            "#,
            "next_pairs_mixed_1k",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("next/pairs bench failed");
            black_box(results);
        });
    });
}

fn register_sort_callback(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("sort_callback_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            local template = {}
            for i = 1, 1000 do
                template[i] = 1001 - i
            end

            local values = {}

            local function refill()
                for i = 1, #template do
                    values[i] = template[i]
                end
            end

            return function()
                refill()
                table.sort(values, function(a, b)
                    local am = a % 10
                    local bm = b % 10
                    if am == bm then
                        return a < b
                    end
                    return am < bm
                end)

                local total = 0
                for i = 1, #values do
                    total = total + values[i]
                end
                return total
            end
            ",
            "sort_callback_1k",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("sort bench failed");
            black_box(results);
        });
    });
}

fn register_list_append_manual(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("list_append_manual_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            return function()
                local values = {}
                local total = 0

                for i = 1, 1000 do
                    values[#values + 1] = i
                end

                for i = 1, #values do
                    total = total + values[i]
                end

                return total
            end
            ",
            "list_append_manual_1k",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("list append manual bench failed");
            black_box(results);
        });
    });
}

fn register_list_append_insert(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("list_append_insert_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            return function()
                local values = {}
                local total = 0

                for i = 1, 1000 do
                    table.insert(values, i)
                end

                for i = 1, #values do
                    total = total + values[i]
                end

                return total
            end
            ",
            "list_append_insert_1k",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("list append insert bench failed");
            black_box(results);
        });
    });
}

fn register_list_remove_tail(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("list_remove_tail_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            local template = {}
            for i = 1, 1000 do
                template[i] = i
            end

            local values = {}

            local function refill()
                for i = 1, #template do
                    values[i] = template[i]
                end
            end

            return function()
                refill()
                local total = 0

                while #values > 0 do
                    total = total + table.remove(values)
                end

                return total
            end
            ",
            "list_remove_tail_1k",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("list remove tail bench failed");
            black_box(results);
        });
    });
}

fn register_list_remove_head(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("list_remove_head_256", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r"
            local template = {}
            for i = 1, 256 do
                template[i] = i
            end

            local values = {}

            local function refill()
                for i = 1, #template do
                    values[i] = template[i]
                end
            end

            return function()
                refill()
                local total = 0

                while #values > 0 do
                    total = total + table.remove(values, 1)
                end

                return total
            end
            ",
            "list_remove_head_256",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("list remove head bench failed");
            black_box(results);
        });
    });
}

fn register_debug_hook_roundtrip(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("hook_roundtrip_200", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::DEBUG).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r#"
            local event_total = 0
            local line_total = 0

            local function leaf(x)
                return x + 1
            end

            local function hop(x)
                return leaf(x) + 1
            end

            local function hook(event, line)
                event_total = event_total + #event
                if line ~= nil then
                    line_total = line_total + line
                end
            end

            return function()
                event_total = 0
                line_total = 0
                local total = 0

                debug.sethook(hook, "crl")
                for i = 1, 200 do
                    local current_hook, mask, count = debug.gethook()
                    if current_hook == hook and mask == "crl" then
                        total = total + count
                    end
                    total = total + hop(i)
                end
                debug.sethook()

                return total + event_total + line_total
            end
            "#,
            "debug_hook_roundtrip",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("debug hook bench failed");
            black_box(results);
        });
    });
}

fn register_loadstring_runtime_errors(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.bench_function("loadstring_runtime_mix_200", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        let bench = load_returned_function(
            &mut lua,
            r#"
            local broken_source = "local x = "

            local function runtime_fail()
                local value = {}
                return value + 1
            end

            return function()
                local total = 0

                for _ = 1, 200 do
                    local compiled, syntax_err = loadstring(broken_source)
                    if compiled ~= nil then
                        error("expected syntax error")
                    end

                    local ok, runtime_err = pcall(runtime_fail)
                    if ok then
                        error("expected runtime error")
                    end

                    total = total + #syntax_err + #runtime_err
                end

                return total
            end
            "#,
            "loadstring_runtime_errors",
        );
        b.iter(|| {
            let results = lua
                .call_function(black_box(&bench), &[])
                .expect("error path bench failed");
            black_box(results);
        });
    });
}

// ---------------------------------------------------------------------------
// State creation
// ---------------------------------------------------------------------------

fn bench_state_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_creation");

    group.bench_function("new_empty", |b| {
        b.iter(|| {
            let lua = Lua::new_empty();
            black_box(lua);
        });
    });

    group.bench_function("new_with_base", |b| {
        b.iter(|| {
            let lua = Lua::new_with(StdLib::BASE).expect("new_with failed");
            black_box(lua);
        });
    });

    group.bench_function("new_full", |b| {
        b.iter(|| {
            let lua = Lua::new().expect("new failed");
            black_box(lua);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

fn bench_compilation(c: &mut Criterion) {
    let mut group = c.benchmark_group("compilation");

    // Minimal script.
    group.bench_function("compile_minimal", |b| {
        b.iter(|| {
            let proto = codegen::compile(black_box(b"return 1"), "bench");
            black_box(proto).expect("compile failed");
        });
    });

    // Arithmetic loop.
    let loop_src = b"local s = 0; for i = 1, 1000 do s = s + i end; return s";
    group.bench_function("compile_loop", |b| {
        b.iter(|| {
            let proto = codegen::compile(black_box(loop_src), "bench");
            black_box(proto).expect("compile failed");
        });
    });

    // Function definitions.
    let funcs_src = br"
        local function fib(n)
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        local function fact(n)
            if n <= 1 then return 1 end
            return n * fact(n - 1)
        end
        return fib(10) + fact(10)
    ";
    group.bench_function("compile_functions", |b| {
        b.iter(|| {
            let proto = codegen::compile(black_box(funcs_src), "bench");
            black_box(proto).expect("compile failed");
        });
    });

    // Table-heavy code.
    let table_src = br#"
        local t = {}
        for i = 1, 100 do
            t[i] = { x = i, y = i * 2, name = "item" .. i }
        end
        local s = 0
        for i = 1, #t do s = s + t[i].x + t[i].y end
        return s
    "#;
    group.bench_function("compile_tables", |b| {
        b.iter(|| {
            let proto = codegen::compile(black_box(table_src), "bench");
            black_box(proto).expect("compile failed");
        });
    });

    register_compile_verybig_chunk(&mut group);

    group.finish();
}

// ---------------------------------------------------------------------------
// VM execution
// ---------------------------------------------------------------------------

fn bench_vm_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("vm_execution");

    // Arithmetic loop.
    group.bench_function("loop_sum_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        b.iter(|| {
            lua.exec(black_box("local s = 0; for i = 1, 1000 do s = s + i end"))
                .expect("exec failed");
        });
    });

    // Recursive fibonacci.
    group.bench_function("fib_20", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        lua.exec("function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end")
            .expect("define fib failed");
        b.iter(|| {
            lua.exec(black_box("fib(20)")).expect("fib failed");
        });
    });

    // String concatenation.
    group.bench_function("string_concat_100", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::STRING).expect("new failed");
        b.iter(|| {
            lua.exec(black_box(
                "local s = ''; for i = 1, 100 do s = s .. 'x' end",
            ))
            .expect("concat failed");
        });
    });

    // Table construction and access.
    group.bench_function("table_build_1k", |b| {
        let mut lua =
            Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        b.iter(|| {
            lua.exec(black_box(
                "local t = {}; for i = 1, 1000 do t[i] = i end; local s = 0; for i = 1, 1000 do s = s + t[i] end",
            ))
            .expect("table failed");
        });
    });

    // Function calls (closure creation + upvalue access).
    group.bench_function("closures_100", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        b.iter(|| {
            lua.exec(black_box(
                r"
                local fns = {}
                for i = 1, 100 do
                    local x = i
                    fns[i] = function() return x end
                end
                local s = 0
                for i = 1, 100 do s = s + fns[i]() end
                ",
            ))
            .expect("closures failed");
        });
    });

    // Method dispatch (metatables).
    group.bench_function("metatable_index_1k", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        lua.exec(
            r"
            local mt = { __index = function(t, k) return k end }
            G_proxy = setmetatable({}, mt)
            ",
        )
        .expect("setup failed");
        b.iter(|| {
            lua.exec(black_box(
                "local p = G_proxy; local s = 0; for i = 1, 1000 do s = s + p[i] end",
            ))
            .expect("meta failed");
        });
    });

    register_control_flow_dispatch(&mut group);
    register_verybig_loaded_chunk(&mut group);

    group.finish();
}

// ---------------------------------------------------------------------------
// Debug library
// ---------------------------------------------------------------------------

fn bench_debug_api(c: &mut Criterion) {
    let mut group = c.benchmark_group("debug_api");

    register_debug_metadata_roundtrip(&mut group);
    register_debug_hook_roundtrip(&mut group);

    group.finish();
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

fn bench_error_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("error_paths");

    register_loadstring_runtime_errors(&mut group);

    group.finish();
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

fn bench_gc(c: &mut Criterion) {
    let mut group = c.benchmark_group("gc");

    // Full GC cycle on a state with many allocations.
    group.bench_function("collect_10k_tables", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        lua.gc_stop();
        // Pre-allocate many tables.
        lua.exec("G_tables = {}; for i = 1, 10000 do G_tables[i] = {i, i+1, i+2} end")
            .expect("alloc failed");
        b.iter(|| {
            lua.gc_collect().expect("gc failed");
        });
    });

    // GC with dead objects (allocation churn).
    group.bench_function("churn_alloc_collect", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        b.iter(|| {
            lua.gc_stop();
            lua.exec(black_box(
                "for i = 1, 1000 do local t = {i, i+1}; local s = 'str' .. i end",
            ))
            .expect("churn failed");
            lua.gc_collect().expect("gc failed");
        });
    });

    // Incremental GC stepping.
    group.bench_function("step_incremental", |b| {
        let mut lua = Lua::new_with(StdLib::BASE).expect("new failed");
        lua.gc_stop();
        lua.exec("G_data = {}; for i = 1, 5000 do G_data[i] = {x=i} end")
            .expect("setup failed");
        b.iter(|| {
            let _ = lua.gc_step(black_box(1024));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// String interning
// ---------------------------------------------------------------------------

fn bench_string_interning(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_interning");

    // Intern unique strings.
    group.bench_function("intern_unique_1k", |b| {
        let mut lua = Lua::new_empty();
        b.iter(|| {
            for i in 0..1000 {
                let s = format!("unique_string_{i}");
                black_box(lua.create_string(s.as_bytes()));
            }
        });
    });

    // Intern duplicate strings (dedup hit).
    group.bench_function("intern_dedup_1k", |b| {
        let mut lua = Lua::new_empty();
        // Pre-intern the strings.
        for i in 0..100 {
            let s = format!("dedup_{i}");
            lua.create_string(s.as_bytes());
        }
        let keys: Vec<String> = (0..100).map(|i| format!("dedup_{i}")).collect();
        b.iter(|| {
            for key in &keys {
                black_box(lua.create_string(key.as_bytes()));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Table operations (via Lua API)
// ---------------------------------------------------------------------------

fn bench_table_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("table_ops");

    // Integer key insert via raw_set.
    group.bench_function("raw_set_int_1k", |b| {
        let mut lua = Lua::new_empty();
        b.iter(|| {
            let table = lua.create_table();
            for i in 1..=1000 {
                lua.table_raw_set(&table, Val::Num(f64::from(i)), Val::Num(f64::from(i)))
                    .expect("raw_set failed");
            }
            black_box(table);
        });
    });

    // String key insert via raw_set.
    group.bench_function("raw_set_str_1k", |b| {
        let mut lua = Lua::new_empty();
        let keys: Vec<Val> = (0..1000)
            .map(|i| {
                let s = format!("key_{i}");
                lua.create_string(s.as_bytes())
            })
            .collect();
        b.iter(|| {
            let table = lua.create_table();
            for (i, key) in keys.iter().enumerate() {
                lua.table_raw_set(&table, *key, Val::Num(i as f64))
                    .expect("raw_set failed");
            }
            black_box(table);
        });
    });

    // Mixed table operations in Lua.
    group.bench_function("mixed_ops_lua", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::TABLE).expect("new failed");
        b.iter(|| {
            lua.exec(black_box(
                r#"
                local t = {}
                for i = 1, 500 do t[i] = i end
                for i = 1, 500 do t["k" .. i] = i end
                local s = 0
                for i = 1, 500 do s = s + t[i] end
                for i = 1, 500 do s = s + t["k" .. i] end
                "#,
            ))
            .expect("mixed ops failed");
        });
    });

    register_list_append_manual(&mut group);
    register_list_append_insert(&mut group);
    register_list_remove_tail(&mut group);
    register_list_remove_head(&mut group);
    register_next_pairs_mixed(&mut group);
    register_sort_callback(&mut group);

    group.finish();
}

// ---------------------------------------------------------------------------
// End-to-end
// ---------------------------------------------------------------------------

fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end");

    // Compile + execute a realistic script.
    group.bench_function("compile_and_run", |b| {
        b.iter(|| {
            let mut lua = Lua::new().expect("new failed");
            lua.exec(black_box(
                r"
                local function map(t, f)
                    local r = {}
                    for i = 1, #t do r[i] = f(t[i]) end
                    return r
                end
                local data = {}
                for i = 1, 100 do data[i] = i end
                local doubled = map(data, function(x) return x * 2 end)
                local sum = 0
                for i = 1, #doubled do sum = sum + doubled[i] end
                ",
            ))
            .expect("exec failed");
        });
    });

    // Coroutine create/resume/yield cycle.
    group.bench_function("coroutine_cycle", |b| {
        let mut lua = Lua::new_with(StdLib::BASE | StdLib::COROUTINE).expect("new failed");
        b.iter(|| {
            lua.exec(black_box(
                r#"
                local co = coroutine.create(function()
                    for i = 1, 50 do
                        coroutine.yield(i)
                    end
                    return 0
                end)
                local s = 0
                while coroutine.status(co) ~= "dead" do
                    local ok, v = coroutine.resume(co)
                    if ok then s = s + v end
                end
                "#,
            ))
            .expect("coroutine failed");
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_state_creation,
    bench_compilation,
    bench_vm_execution,
    bench_debug_api,
    bench_error_paths,
    bench_gc,
    bench_string_interning,
    bench_table_ops,
    bench_end_to_end,
);
criterion_main!(benches);
