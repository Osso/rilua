-- test14.lua: Values and Types - coercion and comparison

-- String-to-number coercion in arithmetic
assert("3" + 5 == 8)
assert("10" - "3" == 7)
assert("2" * "3" == 6)
assert("10" / "2" == 5)
assert("10" % "3" == 1)
assert("2" ^ "10" == 1024)
assert(-"3" == -3)

-- Hex string-to-number coercion
assert("0xff" + 0 == 255)
assert("0XFF" + 0 == 255)

-- Whitespace in string-to-number coercion
assert("  42  " + 0 == 42)

-- Number-to-string coercion in concatenation
assert(3 .. "" == "3")
assert(0 .. "" == "0")
assert(1.5 .. "" == "1.5")

-- String comparison
assert("a" < "b")
assert("b" > "a")
assert("a" <= "a")
assert("a" >= "a")
assert(not ("b" < "a"))
assert("abc" < "abd")
assert("ab" < "abc")
assert(not ("abc" < "abc"))
assert("abc" <= "abc")

-- tonumber
assert(tonumber(42) == 42)
assert(tonumber("42") == 42)
assert(tonumber("  42  ") == 42)
assert(tonumber("0xff") == 255)
assert(tonumber("0XFF") == 255)
assert(tonumber("1.5e2") == 150)
assert(tonumber("hello") == nil)
assert(tonumber(nil) == nil)
assert(tonumber(true) == nil)

-- tonumber with base
assert(tonumber("ff", 16) == 255)
assert(tonumber("10", 2) == 2)
assert(tonumber("77", 8) == 63)
assert(tonumber("10", 36) == 36)
assert(tonumber("ZZ", 36) == 1295)

-- tostring
assert(tostring(42) == "42")
assert(tostring(nil) == "nil")
assert(tostring(true) == "true")
assert(tostring(false) == "false")
assert(type(tostring(42)) == "string")

-- tostring preserves strings
assert(tostring("hello") == "hello")

-- for loop with string values
local sum = 0
for i = "1", "10", "1" do
    sum = sum + i
end
assert(sum == 55)
