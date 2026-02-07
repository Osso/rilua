-- test16.lua: Upvalues and closures (Section 2.3)
-- Note: Tests avoid multiple return values (not yet implemented)

-- Basic closure: inner function captures outer local
local make_counter = function()
    local count = 0
    return function()
        count = count + 1
        return count
    end
end

local c = make_counter()
assert(c() == 1)
assert(c() == 2)
assert(c() == 3)

-- Two independent counters don't share state
local c2 = make_counter()
assert(c2() == 1)
assert(c() == 4)  -- c continues from 3

-- Shared upvalue via table: two closures referencing the same local
local make_pair = function()
    local x = 0
    local t = {}
    t[1] = function() return x end       -- get
    t[2] = function(v) x = v end         -- set
    return t
end

local pair = make_pair()
assert(pair[1]() == 0)
pair[2](42)
assert(pair[1]() == 42)
pair[2](100)
assert(pair[1]() == 100)

-- Upvalue chain: three levels of nesting
local outer = function()
    local x = 10
    local middle = function()
        local inner = function()
            return x
        end
        return inner
    end
    return middle
end
assert(outer()()() == 10)

-- Upvalue mutation through chain (via table)
local outer2 = function()
    local x = 0
    local t = {}
    t[1] = function() x = x + 1 end      -- inc
    t[2] = function() return x end        -- get
    return t
end
local fns = outer2()
assert(fns[2]() == 0)
fns[1]()
assert(fns[2]() == 1)
fns[1]()
fns[1]()
assert(fns[2]() == 3)

-- Upvalue closing in for loop: each iteration gets its own copy
local closures = {}
for i = 1, 3 do
    closures[i] = function() return i end
end
assert(closures[1]() == 1)
assert(closures[2]() == 2)
assert(closures[3]() == 3)

-- Break with upvalues: captured value preserved at break time
local f
for i = 1, 10 do
    if i == 5 then
        f = function() return i end
        break
    end
end
assert(f() == 5)

-- Closure in while loop
local g
local n = 0
while true do
    n = n + 1
    if n == 7 then
        g = function() return n end
        break
    end
end
assert(g() == 7)

-- Multiple upvalues from same scope
local multi_upval = function()
    local a = 10
    local b = 20
    return function() return a + b end
end
assert(multi_upval()() == 30)

-- Upvalue and local with same name in different scopes
-- Inner function has its own local x=2, outer captures x=1
local shadow_outer
local shadow_inner
do
    local x = 1
    shadow_outer = function() return x end
    shadow_inner = function()
        local x = 2
        return x
    end
end
assert(shadow_inner() == 2)
assert(shadow_outer() == 1)

-- Accumulator pattern (parameter as captured upvalue)
local make_adder = function(initial)
    local sum = initial
    return function(n)
        sum = sum + n
        return sum
    end
end
local adder = make_adder(10)
assert(adder(5) == 15)
assert(adder(3) == 18)
assert(adder(2) == 20)
