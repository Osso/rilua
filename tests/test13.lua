-- Binary-safe string length
assert(#"\0" == 1)
assert(#"\255" == 1)
assert(#"\128" == 1)
assert(#"\0\0\0" == 3)
assert(#"\255\255" == 2)
assert(#"hello" == 5)
assert(#"" == 0)
