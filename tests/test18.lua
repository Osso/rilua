-- test18.lua: Method calls, generic for, pairs/next

-- Phase 4: Method call syntax (:)
local obj = {}
function obj:set(v) self.value = v end
function obj:get() return self.value end
obj:set(42)
assert(obj:get() == 42)
assert(obj.value == 42)

-- Method with string arg
local greeter = {}
function greeter:greet(name)
    return "hello " .. name
end
assert(greeter:greet("world") == "hello world")

-- Method with table arg
local collector = {}
function collector:add(t)
    self.items = t
end
collector:add {10, 20, 30}
assert(collector.items[1] == 10)
assert(collector.items[2] == 20)

-- Chained method: function on nested table
local ns = { inner = {} }
function ns.inner:method()
    return self
end
local result = ns.inner:method()
assert(type(result) == "table")

-- Phase 5: Generic for loop with pairs
local sum = 0
local count = 0
for k, v in pairs({a = 1, b = 2, c = 3}) do
    sum = sum + v
    count = count + 1
end
assert(sum == 6)
assert(count == 3)

-- Generic for with single variable
local n = 0
for k in pairs({x = 1, y = 2}) do
    n = n + 1
end
assert(n == 2)

-- Generic for with ipairs
local arr = {10, 20, 30, 40, 50}
local total = 0
local last_i = 0
for i, v in ipairs(arr) do
    total = total + v
    last_i = i
end
assert(total == 150)
assert(last_i == 5)

-- Table length operator
assert(#arr == 5)
assert(#{1, 2, 3} == 3)
assert(#{} == 0)

-- next() directly
local t = {a = 1}
local k, v = next(t)
assert(k == "a")
assert(v == 1)
local k2, v2 = next(t, k)
assert(k2 == nil)

-- next() on empty table
local k3 = next({})
assert(k3 == nil)

print "test18: methods, generic for, pairs all work"
