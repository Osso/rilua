//! Observable local-initialization behavior at control-flow joins.

use super::run_rilua;

#[test]
fn locals_after_guard_start_nil_with_function_or_table_fields() {
    let (stdout, stderr, status) = run_rilua(
        r#"
        local function probe(hb)
            if not (hb and hb.GetEntryAtIndex and hb.GetNumElements) then
                hb = nil
            end
            local nowT, nowG
            return nowT, nowG, hb
        end

        local fixtures = {
            {value = {GetEntryAtIndex = function() end, GetNumElements = function() end}, kept = true},
            {value = {GetEntryAtIndex = {}, GetNumElements = {}}, kept = true},
            {value = {}, kept = false},
            {value = false, kept = false},
        }
        local checked = 0
        for iteration = 1, 4 do
            for _, fixture in ipairs(fixtures) do
                local first, second, retained = probe(fixture.value)
                assert(first == nil, "first local retained " .. type(first))
                assert(second == nil, "second local retained " .. type(second))
                assert(retained == (fixture.kept and fixture.value or nil))
                checked = checked + 1
            end
        end
        print("guard cases", checked)
        "#,
    );
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "guard cases\t16\n");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn disabled_assignment_does_not_reuse_values_from_an_earlier_call() {
    let (stdout, stderr, status) = run_rilua(
        r#"
        local function server_time() return 1000 end
        local function elapsed_time() return 50 end
        local function rebuild(hb, stamp)
            if not (hb and hb.GetEntryAtIndex and hb.GetNumElements) then
                hb = nil
            end
            local nowT, nowG
            if stamp then
                nowT, nowG = server_time(), elapsed_time()
            end
            local entry = hb and hb:GetEntryAtIndex(hb:GetNumElements())
            if nowT then
                return nowT - (nowG - entry.timestamp)
            end
            return "unstamped"
        end

        local buffer = {
            GetEntryAtIndex = function() return {timestamp = 40} end,
            GetNumElements = function() return 1 end,
        }
        for iteration = 1, 5 do
            assert(rebuild(buffer, true) == 990)
            assert(rebuild(buffer, false) == "unstamped")
        end
        print("timestamp branches", 10)
        "#,
    );
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "timestamp branches\t10\n");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn locals_after_elseif_join_do_not_inherit_condition_values() {
    let (stdout, stderr, status) = run_rilua(
        r#"
        local function probe(selector, value)
            if selector == 1 then
                value = nil
            elseif value and value.clear and value.clear() then
                value = nil
            end
            local first, second
            return first, second, value
        end

        local keep = {clear = function() return false end}
        local clear = {clear = function() return true end}
        for iteration = 1, 4 do
            for _, fixture in ipairs({
                {selector = 1, value = keep},
                {selector = 2, value = keep, expected = keep},
                {selector = 2, value = clear},
            }) do
                local first, second, retained = probe(fixture.selector, fixture.value)
                assert(first == nil, "first join local retained " .. type(first))
                assert(second == nil, "second join local retained " .. type(second))
                assert(retained == fixture.expected)
            end
        end
        print("elseif cases", 12)
        "#,
    );
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "elseif cases\t12\n");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn loop_exits_initialize_fresh_locals() {
    let (stdout, stderr, status) = run_rilua(
        r#"
        local function probe(limit, stop)
            local count, value = 0, {}
            while count < limit do
                count = count + 1
                if count == stop then
                    value = nil
                    break
                end
            end
            local first, second
            return first, second, count, value
        end

        for _, fixture in ipairs({
            {limit = 0, stop = 1, count = 0, cleared = false},
            {limit = 4, stop = 2, count = 2, cleared = true},
            {limit = 3, stop = 4, count = 3, cleared = false},
        }) do
            local first, second, count, value = probe(fixture.limit, fixture.stop)
            assert(first == nil and second == nil)
            assert(count == fixture.count)
            assert((value == nil) == fixture.cleared)
        end
        print("loop exits", 3)
        "#,
    );
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "loop exits\t3\n");
    assert!(stderr.is_empty(), "{stderr}");
}
