use rilua::Result;
use rilua::State;

fn run_file(filename: &str) -> Result<()> {
    let mut state = State::new();
    state.do_file(filename)
}

#[test]
fn test01() -> Result<()> {
    run_file("tests/test01.lua")
}

#[test]
fn test02() -> Result<()> {
    run_file("tests/test02.lua")
}

#[test]
fn test03() -> Result<()> {
    run_file("tests/test03.lua")
}

#[test]
fn test04() -> Result<()> {
    run_file("tests/test04.lua")
}

#[test]
fn test05() -> Result<()> {
    run_file("tests/test05.lua")
}

#[test]
fn test06() -> Result<()> {
    run_file("tests/test06.lua")
}

#[test]
fn test07() -> Result<()> {
    run_file("tests/test07.lua")
}

#[test]
fn test08() -> Result<()> {
    run_file("tests/test08.lua")
}

#[test]
fn test09() -> Result<()> {
    run_file("tests/test09.lua")
}

#[test]
fn test10() -> Result<()> {
    run_file("tests/test10.lua")
}

#[test]
fn test11() -> Result<()> {
    run_file("tests/test11.lua")
}

#[test]
fn test12() -> Result<()> {
    run_file("tests/test12.lua")
}

// PUC-Rio Lua 5.1.1 official test suite (verbatim copies)
// Source: ~/Repos/github.com/lua/tests at tag v5_1_1

#[test]
#[ignore]
fn lua51_constructs() -> Result<()> {
    run_file("tests/lua51/constructs.lua")
}

#[test]
#[ignore]
fn lua51_locals() -> Result<()> {
    run_file("tests/lua51/locals.lua")
}

#[test]
#[ignore]
fn lua51_attrib() -> Result<()> {
    run_file("tests/lua51/attrib.lua")
}

#[test]
#[ignore]
fn lua51_math() -> Result<()> {
    run_file("tests/lua51/math.lua")
}

#[test]
#[ignore]
fn lua51_nextvar() -> Result<()> {
    run_file("tests/lua51/nextvar.lua")
}

#[test]
#[ignore]
fn lua51_literals() -> Result<()> {
    run_file("tests/lua51/literals.lua")
}

#[test]
#[ignore]
fn lua51_strings() -> Result<()> {
    run_file("tests/lua51/strings.lua")
}

#[test]
#[ignore]
fn lua51_calls() -> Result<()> {
    run_file("tests/lua51/calls.lua")
}

#[test]
#[ignore]
fn lua51_closure() -> Result<()> {
    run_file("tests/lua51/closure.lua")
}

#[test]
#[ignore]
fn lua51_events() -> Result<()> {
    run_file("tests/lua51/events.lua")
}

#[test]
#[ignore]
fn lua51_vararg() -> Result<()> {
    run_file("tests/lua51/vararg.lua")
}

#[test]
#[ignore]
fn lua51_errors() -> Result<()> {
    run_file("tests/lua51/errors.lua")
}

#[test]
#[ignore]
fn lua51_gc() -> Result<()> {
    run_file("tests/lua51/gc.lua")
}

#[test]
#[ignore]
fn lua51_sort() -> Result<()> {
    run_file("tests/lua51/sort.lua")
}

#[test]
#[ignore]
fn lua51_pm() -> Result<()> {
    run_file("tests/lua51/pm.lua")
}
