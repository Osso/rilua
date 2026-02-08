-- test17.lua: Multiple return values, local function, unparenthesized calls

-- Phase 1: Multiple return values
local function two() return 1, 2 end
local a, b = two()
assert(a == 1)
assert(b == 2)

-- Extra returns discarded
local c = two()
assert(c == 1)

-- Missing returns filled with nil
local function one() return 1 end
local d, e = one()
assert(d == 1)
assert(e == nil)

-- Multiple returns from nested calls
local function three() return 10, 20, 30 end
local x, y, z = three()
assert(x == 10)
assert(y == 20)
assert(z == 30)

-- Return in table constructor (single value only for now;
-- multi-return expansion in constructors requires the multi-return protocol)
local t = {three()}
assert(t[1] == 10)

-- Phase 2: local function (recursion)
local function factorial(n)
    if n <= 1 then return 1 end
    return n * factorial(n - 1)
end
assert(factorial(1) == 1)
assert(factorial(5) == 120)
assert(factorial(10) == 3628800)

-- local function with upvalue capture
local function make_counter()
    local count = 0
    local function inc()
        count = count + 1
        return count
    end
    return inc
end
local counter = make_counter()
assert(counter() == 1)
assert(counter() == 2)
assert(counter() == 3)

-- Phase 3: Unparenthesized function calls
assert(type "hello" == "string")
assert(type "world" == "string")
assert(type {1, 2, 3} == "table")

-- print with string arg (should not error)
print "test17: unparenthesized calls work"
