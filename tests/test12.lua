-- Test: comments and long brackets
-- Covers: short comments, long comments (all levels), long strings (all levels),
-- edge cases from Lua 5.1.1 Reference Manual section 2.1
--
-- Note: short string escape sequences (\n, \t, \\) are not yet implemented.
-- Tests avoid comparing long strings containing newlines against short strings
-- that would require escape processing.

-- ============================================================
-- Short comments
-- ============================================================

-- Short comment: everything after -- to end of line is ignored
x = 1 -- this is a comment
assert(x == 1)

-- Short comment on its own line
-- another one
x = 2
assert(x == 2)

-- Empty short comment
--
x = 3
assert(x == 3)

-- ============================================================
-- Long comments (level 0)
-- ============================================================

--[[ This is a long comment ]]
x = 10
assert(x == 10)

--[[ Multi-line
long comment
spanning several lines ]]
x = 11
assert(x == 11)

-- Empty long comment
--[[]]
x = 12
assert(x == 12)

-- Long comment with ]] inside at different level does not close it
--[=[ This contains ]] but keeps going ]=]
x = 13
assert(x == 13)

-- ============================================================
-- Long comments (higher levels)
-- ============================================================

--[=[ Level 1 long comment ]=]
x = 20
assert(x == 20)

--[==[ Level 2 long comment ]==]
x = 21
assert(x == 21)

--[===[ Level 3 long comment ]===]
x = 22
assert(x == 22)

-- Long comment containing close brackets of other levels
--[==[ Contains ]] and ]=] inside without closing ]==]
x = 23
assert(x == 23)

-- ============================================================
-- Short comment that looks like long but isn't
-- ============================================================

-- A `--[` without a second `[` is just a short comment
--[ this is still a short comment
x = 30
assert(x == 30)

-- `--[=` without closing `[` is also a short comment
--[= this is still a short comment
x = 31
assert(x == 31)

-- `--[==` without closing `[` is also a short comment
--[== this is still a short comment
x = 32
assert(x == 32)

-- ============================================================
-- Long strings (level 0)
-- ============================================================

x = [[hello]]
assert(x == "hello")

x = [[]]
assert(x == "")

-- Long string spanning multiple lines: verify by length
-- "line1" (5) + newline (1) + "line2" (5) + newline (1) + "line3" (5) = 17
x = [[line1
line2
line3]]
assert(#x == 17)

-- Leading newline after [[ is stripped (Lua 5.1 spec)
x = [[
hello]]
assert(x == "hello")

-- Leading newline stripped: only the first newline
-- Content is: newline + "hello" = 6 chars
x = [[

hello]]
assert(#x == 6)

-- ============================================================
-- Long strings (higher levels)
-- ============================================================

x = [=[hello]=]
assert(x == "hello")

x = [==[hello]==]
assert(x == "hello")

x = [===[hello]===]
assert(x == "hello")

-- Level 1 string containing ]] (level 0 close bracket, should not close it)
x = [=[contains ]] inside]=]
assert(x == "contains ]] inside")

-- Level 0 string containing ]=] (level 1 close bracket, should not close it)
x = [[contains ]=] inside]]
assert(x == "contains ]=] inside")

-- Level 2 string containing ]] and ]=] (neither should close it)
x = [==[contains ]] and ]=] inside]==]
assert(x == "contains ]] and ]=] inside")

-- ============================================================
-- Long strings: leading newline behavior at higher levels
-- ============================================================

x = [=[
hello]=]
assert(x == "hello")

x = [==[
hello]==]
assert(x == "hello")

-- Only the first newline is stripped; second is content
-- Content: newline + "hello" = 6 chars
x = [=[

hello]=]
assert(#x == 6)

-- No newline to strip
x = [=[hello]=]
assert(x == "hello")

-- ============================================================
-- Long strings: no escape sequence processing
-- ============================================================

-- Backslash sequences are literal in long strings.
-- Compare long string against itself to verify content preserved.
x = [[hello\nworld]]
assert(x == [[hello\nworld]])

x = [[hello\tworld]]
assert(x == [[hello\tworld]])

-- Backslash-n in a long string is NOT a newline.
-- "hello\nworld" with literal \n is 12 chars: h,e,l,l,o,\,n,w,o,r,l,d
assert(#[[hello\nworld]] == 12)

-- ============================================================
-- Long strings used in expressions
-- ============================================================

-- Concatenation with long strings
x = [[hello]] .. [[ ]] .. [[world]]
assert(x == "hello world")

-- Long string in comparison
assert([[abc]] == "abc")

-- Long string as table value
t = {}
t["key"] = [[value]]
assert(t["key"] == "value")

-- ============================================================
-- Comments and code on the same line
-- ============================================================

x = 100 --[[ inline long comment ]] + 1
assert(x == 101)

-- Long comment between expressions
x = 50 --[[ comment ]] + 50
assert(x == 100)

-- ============================================================
-- Nested brackets in long strings
-- ============================================================

-- [[ inside a level-1 string does not start nesting
x = [=[one [[ two ]] three]=]
assert(x == "one [[ two ]] three")

-- [=[ inside a level-2 string does not start nesting
x = [==[one [=[ two ]=] three]==]
assert(x == "one [=[ two ]=] three")

-- ============================================================
-- Edge: long comment immediately before EOF (no trailing newline)
-- ============================================================

x = 999
assert(x == 999)
--[[ final comment with no newline after it ]]