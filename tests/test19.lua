-- test19.lua: Expressions -- modulo, varargs, multi-return expansion, select

-- Phase 1: Modulo with floor division semantics
assert(5 % 3 == 2)
assert(-5 % 3 == 1)
assert(5 % -3 == -1)
assert(-5 % -3 == -2)
assert(1.5 % 1 == 0.5)

-- Phase 2: Varargs
local function f(...) return ... end
local a, b, c = f(1, 2, 3)
assert(a == 1)
assert(b == 2)
assert(c == 3)

-- Single vararg
local function first(...) local a = ... return a end
assert(first(42) == 42)

-- Vararg forwarding
local function passthrough(...) return f(...) end
local x, y = passthrough(10, 20)
assert(x == 10)
assert(y == 20)

-- Vararg count via select
local function count(...) return select('#', ...) end
assert(count() == 0)
assert(count(1) == 1)
assert(count(1, 2, 3) == 3)

-- Phase 3: Multi-return in table constructor
local function three() return 10, 20, 30 end
local t = {three()}
assert(t[1] == 10)
assert(t[2] == 20)
assert(t[3] == 30)

-- Mixed fixed + multi-return
local t2 = {1, 2, three()}
assert(t2[1] == 1)
assert(t2[2] == 2)
assert(t2[3] == 10)
assert(t2[4] == 20)
assert(t2[5] == 30)

-- Vararg in table constructor
local function pack(...) return {...} end
local t3 = pack(4, 5, 6)
assert(t3[1] == 4)
assert(t3[2] == 5)
assert(t3[3] == 6)

-- Phase 4: Multi-return in function call arguments
local function sum3(a, b, c) return a + b + c end
local function two() return 10, 20 end
assert(sum3(1, two()) == 31)

-- Only last arg expands
local function identity(a) return a end
assert(identity(two()) == 10)

-- Nested multi-return calls
assert(type(tostring(42)) == "string")

-- Phase 5: select()
assert(select('#') == 0)
assert(select('#', 1, 2, 3) == 3)
assert(select(1, 10, 20, 30) == 10)
assert(select(2, 10, 20, 30) == 20)
assert(select(3, 10, 20, 30) == 30)

print "test19: expressions (modulo, varargs, multi-return, select) all work"
