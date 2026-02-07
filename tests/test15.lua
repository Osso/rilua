-- break in while
local x = 0
while true do x = x + 1; if x == 5 then break end end
assert(x == 5)

-- break in repeat
local y = 0
repeat y = y + 1; if y == 3 then break end until false
assert(y == 3)

-- break in numeric for
local z = 0
for i = 1, 100 do z = i; if i == 10 then break end end
assert(z == 10)

-- break in nested loops (only breaks innermost)
local a = 0
local b = 0
for i = 1, 3 do
    a = a + 1
    for j = 1, 100 do
        b = b + 1
        if j == 2 then break end
    end
end
assert(a == 3)
assert(b == 6)

-- break through do block inside loop
local c = 0
while true do
    do
        c = c + 1
        if c == 4 then break end
    end
end
assert(c == 4)
