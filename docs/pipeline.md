# Compilation Pipeline

## Decision

Lexer -> Parser -> AST -> Compiler -> Proto (bytecode)

A multi-phase compilation pipeline with an explicit AST intermediate
representation, following the approach used by Luau.

## Context

PUC-Rio Lua 5.1.1 uses a single-pass recursive descent compiler that
emits bytecode directly during parsing. There is no AST — the parser
and code generator are interleaved in `lparser.c` and `lcode.c`.

This is efficient but tightly couples parsing and code generation.
Bugs in one phase are hard to isolate. Testing requires running the
full pipeline. Adding optimizations beyond constant folding requires
changes to the interleaved parse-and-emit code.

Luau (Roblox's Lua 5.1-compatible scripting language) uses an explicit
AST phase. The parser produces `AstStatBlock` trees. A separate
compiler walks the AST and emits bytecode.

## Phases

### Phase 1: Lexer (source -> tokens)

The lexer converts source text into a stream of tokens. Each token
has a type, optional value (for literals), and source location
(line, column).

Responsibilities:

- Character-level scanning
- Keyword recognition
- Number literal parsing (decimal, hex, float)
- String literal parsing (short strings with escapes, long strings
  with bracket notation)
- Comment handling (single-line, long comments)
- Whitespace skipping
- Source position tracking

The lexer provides one-token lookahead via a `peek()` method.

#### Token Types

21 reserved words, 6 multi-character operators, 3 literal tokens,
and 1 end-of-stream token. Single-character tokens (`+`, `-`, `(`,
etc.) are represented by their byte value.

**Reserved words** (case-sensitive):

`and`, `break`, `do`, `else`, `elseif`, `end`, `false`, `for`,
`function`, `if`, `in`, `local`, `nil`, `not`, `or`, `repeat`,
`return`, `then`, `true`, `until`, `while`

**Multi-character operators**: `..` (concat), `...` (vararg),
`==`, `>=`, `<=`, `~=`

**Literal tokens**: `TK_NUMBER`, `TK_STRING`, `TK_NAME` (identifier)

Keywords are recognized by interning the identifier string and
checking a `reserved` flag on the interned entry. Matching is
case-sensitive.

#### Number Literal Parsing

The lexer accepts decimal, float, scientific, and hexadecimal
number literals.

| Format | Examples | Method |
|--------|----------|--------|
| Decimal | `123`, `0` | strtod equivalent |
| Float | `123.456`, `.5`, `123.` | strtod equivalent |
| Scientific | `1e10`, `1.5e-5`, `123E+2` | strtod equivalent |
| Hexadecimal | `0xff`, `0xFF` | `u64` parse base 16 |

Parsing algorithm:

1. Consume digits and up to one decimal point.
2. If `e` or `E` follows, consume it and an optional `+` or `-`.
3. Consume any trailing alphanumeric characters (causes parse
   failure later if present).
4. Convert via `str::parse::<f64>()` or equivalent. If that fails at
   an `x`/`X` character, retry as hexadecimal.

Leading dot: `.5` is handled by the main lexer loop. When `.` is
followed by a digit, the lexer delegates to number parsing.

Locale handling: PUC-Rio respects the locale's decimal separator.
rilua uses `.` unconditionally (Rust's `f64` parsing is
locale-independent).

#### String Literal Parsing

Single-quoted (`'`) and double-quoted (`"`) strings are identical in
functionality. The delimiter character must match.

**Escape sequences:**

| Sequence | Result | Code |
|----------|--------|------|
| `\a` | Bell | 0x07 |
| `\b` | Backspace | 0x08 |
| `\f` | Form feed | 0x0C |
| `\n` | Newline | 0x0A |
| `\r` | Carriage return | 0x0D |
| `\t` | Tab | 0x09 |
| `\v` | Vertical tab | 0x0B |
| `\\` | Backslash | 0x5C |
| `\"` | Double quote | 0x22 |
| `\'` | Single quote | 0x27 |
| `\<newline>` | Newline (literal) | 0x0A |
| `\ddd` | Decimal byte value | 0-255 |

Numeric escapes (`\ddd`) consume up to 3 decimal digits. Maximum
value 255 (`UCHAR_MAX`). Values above 255 are a lexer error.

Unknown escape sequences: the backslash is discarded and the next
character is kept as-is (e.g., `\?` becomes `?`).

Bare newlines inside short strings are an error ("unfinished
string").

#### Long String and Comment Parsing

Long strings use bracket notation: `[==[...]==]` where the number
of `=` signs must match between opening and closing brackets.

- `[[...]]` — level 0
- `[=[...]=]` — level 1
- `[==[...]==]` — level 2, etc.

Behavior:

- First newline after opening bracket is stripped.
- All newlines are normalized: `\r\n` and `\n\r` become `\n`,
  bare `\r` becomes `\n`.
- No escape sequences are processed inside long strings.
- Different bracket levels do not interfere: `[==[` can contain
  `[[...]]` without issue.

Long comments use the same bracket notation preceded by `--`:
`--[==[...]==]`. Content is parsed but discarded.

Short comments: `--` to end of line. The lexer distinguishes
`--[[` (long comment) from `--[` (short comment) by checking
whether `skip_sep()` finds a valid bracket-equals pattern.

#### Source Position Tracking

The lexer maintains two line counters:

- `line` — current line being read (updated on every newline)
- `last_line` — line of the most recently consumed token

Both start at 1. `\n`, `\r`, `\r\n`, and `\n\r` are all recognized
as single newlines. Line numbers are stored in the Proto's `lineinfo`
array (one entry per instruction).

### Phase 2: Parser (tokens -> AST)

The parser consumes tokens and produces an abstract syntax tree.
It is a hand-written recursive descent parser following Lua 5.1.1
grammar rules.

Responsibilities:

- Statement parsing (if, while, for, repeat, do, return, break,
  assignment, function call, local declaration)
- Expression parsing with operator precedence
- Block/scope tracking
- Syntax error reporting with source locations

The parser does NOT:

- Resolve variable scopes (that is the compiler's job)
- Emit bytecode
- Perform optimizations

#### AST Node Types

The AST uses Rust enums for statements and expressions. Each node
carries a `Span` (line range) for error reporting.

**Statements** (`Stat` enum):

| Variant | Lua syntax | Fields |
|---------|-----------|--------|
| `Assign` | `a, b = x, y` | targets: `Vec<Expr>`, values: `Vec<Expr>` |
| `Do` | `do ... end` | body: `Block` |
| `While` | `while e do ... end` | condition: `Expr`, body: `Block` |
| `Repeat` | `repeat ... until e` | body: `Block`, condition: `Expr` |
| `If` | `if ... elseif ... else ... end` | branches: `Vec<(Expr, Block)>`, else_body: `Option<Block>` |
| `NumericFor` | `for i=a,b,c do ... end` | name: `Name`, start: `Expr`, limit: `Expr`, step: `Option<Expr>`, body: `Block` |
| `GenericFor` | `for k,v in iter do ... end` | names: `Vec<Name>`, iterators: `Vec<Expr>`, body: `Block` |
| `Return` | `return a, b` | values: `Vec<Expr>` |
| `Break` | `break` | (none) |
| `FuncDecl` | `function f() ... end` | name: `FuncName`, body: `FuncBody` |
| `LocalFunc` | `local function f() ... end` | name: `Name`, body: `FuncBody` |
| `LocalDecl` | `local a, b = x, y` | names: `Vec<Name>`, values: `Vec<Expr>` |
| `ExprStat` | `f()` or `a:b()` | expr: `Expr` (must be Call or MethodCall) |

**Expressions** (`Expr` enum):

| Variant | Lua syntax | Fields |
|---------|-----------|--------|
| `Nil` | `nil` | |
| `True` | `true` | |
| `False` | `false` | |
| `Number` | `3.14` | value: `f64` |
| `String` | `"hello"` | value: `String` |
| `VarArg` | `...` | |
| `Name` | `x` | name: `String` |
| `BinOp` | `a + b` | op: `BinOp`, left: `Box<Expr>`, right: `Box<Expr>` |
| `UnOp` | `-a`, `not a`, `#a` | op: `UnOp`, operand: `Box<Expr>` |
| `Index` | `t[k]` | table: `Box<Expr>`, key: `Box<Expr>` |
| `Field` | `t.k` | table: `Box<Expr>`, name: `Name` |
| `MethodCall` | `t:m(args)` | table: `Box<Expr>`, method: `Name`, args: `Vec<Expr>` |
| `Call` | `f(args)` | func: `Box<Expr>`, args: `Vec<Expr>` |
| `FuncDef` | `function(...) end` | body: `FuncBody` |
| `TableCtor` | `{a, [k]=v, f=1}` | fields: `Vec<Field>` |

**Supporting types:**

```rust
type Block = Vec<Stat>;
type Name = String;

enum BinOp {
    Add, Sub, Mul, Div, Mod, Pow,      // arithmetic
    Concat,                              // ..
    Lt, Le, Gt, Ge, Eq, Ne,            // comparison
    And, Or,                             // logical
}

enum UnOp {
    Neg, Not, Len,                       // - not #
}

enum Field {
    IndexField { key: Expr, value: Expr },  // [k] = v
    NameField { name: Name, value: Expr },  // name = v
    ValueField { value: Expr },             // v (array-style)
}

struct FuncBody {
    params: Vec<Name>,
    has_varargs: bool,
    body: Block,
}

struct FuncName {
    parts: Vec<Name>,   // a.b.c
    method: Option<Name>, // :m
}

struct Span {
    line: u32,
    last_line: u32,
}
```

**Operator precedence** (lowest to highest):

1. `or`
2. `and`
3. `<  >  <=  >=  ~=  ==`
4. `..` (right-associative)
5. `+  -`
6. `*  /  %`
7. `not  #  -` (unary)
8. `^` (right-associative)

#### Parsing Algorithms

**Pratt parsing (operator-precedence climbing)** is used for
expression parsing. The `subexpr(limit)` function takes a minimum
priority and combines operators whose left priority exceeds the limit.

Grammar rule:

```text
subexpr -> (simpleexp | unop subexpr) { binop subexpr }
```

**Priority table** (left, right priorities per operator):

| Operator | Left | Right | Associativity |
|----------|------|-------|---------------|
| `or` | 1 | 1 | left |
| `and` | 2 | 2 | left |
| `<  >  <=  >=  ~=  ==` | 3 | 3 | left |
| `..` | 5 | 4 | right |
| `+  -` | 6 | 6 | left |
| `*  /  %` | 7 | 7 | left |
| `not  #  -` (unary) | -- | 8 | unary |
| `^` | 10 | 9 | right |

Right-associative operators have `left > right`. When parsing `2^3^4`,
after consuming the first `^` (left=10), the recursive call uses
right=9 as the limit. The next `^` has left=10 > 9, so it is consumed
in the recursion, producing `2^(3^4)`. For left-associative operators
like `+` (left=6, right=6), the next `+` has left=6 which is NOT
greater than limit 6, so it is not consumed in recursion, producing
`(a+b)+c`.

**Statement dispatch** parses one statement per call and returns 1 if
it must be the last statement in a block (return, break), 0 otherwise:

| Token | Production | Last? |
|-------|-----------|-------|
| `if` | if-elseif-else-end chain | no |
| `while` | while-do-end loop | no |
| `do` | do-end block | no |
| `for` | numeric or generic for loop | no |
| `repeat` | repeat-until loop | no |
| `function` | function declaration | no |
| `local` | local declaration or local function | no |
| `return` | return statement | **yes** |
| `break` | break statement | **yes** |
| default | function call or assignment | no |

The enclosing `chunk()` loop terminates when a statement returns
"last" or the current token is a block-follower (`else`, `elseif`,
`end`, `until`, or end-of-stream).

**Assignment parsing** uses a recursive linked list. `exprstat()`
parses a `primaryexp()`. If the result is a call, it is a call
statement. Otherwise it starts the `assignment()` chain:

1. Validate the target is `VLOCAL`, `VUPVAL`, `VGLOBAL`, or
   `VINDEXED`. Anything else is a syntax error.
2. If comma follows, parse the next target and recurse.
3. If `=` follows, parse the expression list.
4. Stores happen in reverse order as recursion unwinds.

Conflict detection: in `a, t[a] = x, y`, the local `a` is both
assigned and used as a table index. The parser copies `a`'s current
value to a temporary register before assignment to avoid corruption.

**Table constructor parsing** tracks three field forms:

| Token | Form | Action |
|-------|------|--------|
| `TK_NAME` followed by `=` | `name = expr` | Record field, emits `OP_SETTABLE` |
| `[` | `[expr] = expr` | Record field, emits `OP_SETTABLE` |
| other | positional value | List field, batched into `OP_SETLIST` |

List fields are flushed in batches of `LFIELDS_PER_FLUSH` (50). The
last list element gets special handling for multi-return calls or
varargs. The initial `OP_NEWTABLE` instruction is backpatched with
final array/hash size hints.

**Function call ambiguity**: when `(` appears on a different line
than the function expression, the parser warns about ambiguous syntax
(`function call x new statement`). The `f{...}` and `f"string"` call
forms are also supported.

**For loop disambiguation**: after `for name`, if `=` follows it is a
numeric for (`for i=a,b,c do...end`), if `,` or `in` follows it is a
generic for (`for k,v in iter do...end`).

**Local function** (`local function f() end`) registers the variable
before parsing the body, enabling self-recursion. The debug info
`startpc` is patched after the closure is stored.

**Error conditions and limits**:

| Limit | Value | Error message |
|-------|-------|---------------|
| Local variables per function | 200 | `"too many local variables"` |
| Upvalues per function | 60 | `"function has more than 60 upvalues"` |
| Syntax nesting depth | 200 | `"chunk has too many syntax levels"` |
| Constructor array items | 2^18-1 | `"items in a constructor"` |

The parser does no error recovery. Every syntax error terminates
parsing immediately. Mismatched block delimiters report the opening
line: `"'end' expected (to close 'if' at line N)"`.

#### Grammar Reference

```text
chunk       -> { stat [';'] }
block       -> chunk
stat        -> ifstat | whilestat | DO block END |
               forstat | repeatstat | funcstat |
               localstat | retstat | breakstat | exprstat
ifstat      -> IF cond THEN block
               {ELSEIF cond THEN block} [ELSE block] END
whilestat   -> WHILE cond DO block END
repeatstat  -> REPEAT block UNTIL cond
forstat     -> FOR (fornum | forlist) END
fornum      -> NAME '=' exp ',' exp [',' exp] DO block
forlist     -> NAME {',' NAME} IN explist DO block
funcstat    -> FUNCTION funcname body
funcname    -> NAME {'.' NAME} [':' NAME]
localstat   -> LOCAL NAME {',' NAME} ['=' explist]
localfunc   -> LOCAL FUNCTION NAME body
retstat     -> RETURN [explist]
breakstat   -> BREAK
exprstat    -> primaryexp (call | assignment)
assignment  -> ',' primaryexp assignment | '=' explist
cond        -> expr
expr        -> subexpr(0)
subexpr     -> (simpleexp | unop subexpr) {binop subexpr}
simpleexp   -> NUMBER | STRING | NIL | TRUE | FALSE | '...' |
               constructor | FUNCTION body | primaryexp
primaryexp  -> prefixexp {'.' NAME | '[' expr ']' |
               ':' NAME funcargs | funcargs}
prefixexp   -> NAME | '(' expr ')'
funcargs    -> '(' [explist] ')' | constructor | STRING
explist     -> expr {',' expr}
constructor -> '{' [fieldlist] '}'
fieldlist   -> field {(',' | ';') field} [(',' | ';')]
field       -> '[' expr ']' '=' expr | NAME '=' expr | expr
parlist     -> [param {',' param}]
param       -> NAME | '...'
body        -> '(' parlist ')' chunk END
```

### Phase 3: Compiler (AST -> Proto)

The compiler walks the AST and emits register-based bytecode into
Proto structures.

Responsibilities:

- Variable resolution (locals, upvalues, globals)
- Register allocation
- Instruction emission
- Constant pool management
- Jump backpatching
- Nested function compilation (recursive Proto creation)
- Debug information (line numbers, local names)

The compiler maintains a `FuncState` for each function being
compiled, tracking:

- `freereg` — next available register
- `nactvar` — number of active local variables
- Local variable declarations and scopes
- Upvalue resolution chain
- Constant table

### Output: Proto

The compilation output is a `Proto` (function prototype) containing:

- Bytecode instruction array
- Constant pool (nil, booleans, numbers, strings)
- Nested Proto array (for inner function definitions)
- Upvalue descriptors
- Debug information (line map, local variable names)
- Function metadata (parameter count, vararg flag, max stack size)

Proto is immutable after compilation and shared between closures
via `Rc<Proto>`.

#### Proto Fields

```rust
struct Proto {
    // Code and constants
    code: Vec<Instruction>,          // Bytecode array
    constants: Vec<Val>,             // Constant pool (nil, bool, f64, string)
    protos: Vec<Rc<Proto>>,          // Nested function prototypes

    // Debug info
    line_info: Vec<i32>,             // Source line per instruction (1:1)
    local_vars: Vec<LocalVar>,       // Local variable names and scopes
    upvalue_names: Vec<String>,      // Upvalue names for debug
    source: String,                  // Source file name

    // Function metadata
    line_defined: u32,               // Line where function starts (0 for main)
    last_line_defined: u32,          // Line where function ends
    num_upvalues: u8,                // Count of upvalues (0-255)
    num_params: u8,                  // Count of fixed parameters (0-255)
    is_vararg: u8,                   // VARARG_* flags (bitwise OR)
    max_stack_size: u8,              // Max registers used (0-255)
}

struct LocalVar {
    name: String,
    start_pc: usize,  // First instruction where variable is active
    end_pc: usize,    // First instruction where variable is dead
}
```

**Constants** can be exactly four types: nil, boolean, number
(`f64`), and string. They are indexed by the RK encoding in
instructions.

**Line info** is a flat array with one entry per instruction
(same length as `code`). `line_info[pc]` gives the source line for
instruction `pc`. Not compressed.

**Local variables** are active during `start_pc <= pc < end_pc`.
Used by the debug library (`debug.getlocal`) and for error messages
that name variables.

**Vararg flags:**

| Flag | Value | Meaning |
|------|-------|---------|
| `VARARG_HASARG` | 1 | Function has legacy `arg` table |
| `VARARG_ISVARARG` | 2 | Function uses `...` |
| `VARARG_NEEDSARG` | 4 | Runtime creates `arg` table |

Flags are combined with OR. `VARARG_NEEDSARG` implies
`VARARG_HASARG`. The main chunk always has `VARARG_ISVARARG`.

#### FuncState (Compiler State)

During compilation, the compiler maintains a `FuncState` per
function being compiled:

```rust
struct FuncState {
    proto: Proto,                 // Proto being built
    parent: Option<&FuncState>,   // Enclosing function (for upvalue resolution)
    pc: usize,                    // Next instruction index to write
    free_reg: u8,                 // First free register
    num_active_vars: u8,          // Currently active local variables
    active_vars: Vec<u16>,        // Stack of indices into proto.local_vars
    upvalues: Vec<UpvalDesc>,     // Upvalue resolution info
}

struct UpvalDesc {
    in_stack: bool,   // true = captures parent's local; false = chains parent's upvalue
    index: u8,        // Register index (if in_stack) or parent upvalue index
}
```

The `upvalues` array in `FuncState` tracks how each upvalue is
resolved at compile time. This information drives the
pseudo-instructions emitted after `OP_CLOSURE` (MOVE for locals,
GETUPVAL for chained upvalues). Only the names are stored in the
final Proto.

#### Register Allocation

The compiler uses a simple stack-based register allocator via the
`free_reg` counter:

- `free_reg` is the next available register. It starts at 0 and
  increases as locals are declared or temporary values are needed.
- Local variables occupy the lowest registers (register `i` holds
  local variable `i`). `num_active_vars` tracks how many locals exist.
- Temporary values are allocated above the locals. After use, `free_reg`
  is decremented to release them.
- Maximum register count per function: `MAXSTACK` = 250.

#### Constant Pool and RK Optimization

Constants (nil, boolean, number, string) are stored in the Proto's
`constants` array. Instructions that accept RK operands (9-bit B or
C fields) can reference constants directly when the constant index
is 255 or less:

- Bit 8 set (values 256-511): constant index 0-255
- Bit 8 clear (values 0-255): register number

This avoids loading constants into registers for common patterns
like `x + 1`, `t["key"]`, or `a == "hello"`. When a constant index
exceeds 255, the value must be loaded into a register first via
`OP_LOADK`.

Instructions using RK operands: `ADD`, `SUB`, `MUL`, `DIV`, `MOD`,
`POW` (both B and C), `EQ`, `LT`, `LE` (both B and C), `GETTABLE`
(C only), `SETTABLE` (both B and C), `SELF` (C only).

#### Jump Backpatching

The compiler uses linked lists threaded through JMP instruction sBx
fields for forward references:

- `NO_JUMP` (-1, encoded as 0 in unsigned sBx) terminates a list.
- Each unresolved jump stores the offset to the next jump in the
  list within its sBx field.
- `concat(list1, list2)` appends list2 to the tail of list1.
- `patchtohere(list)` defers resolution: jumps are concatenated onto
  a pending list (`jpc`) that is resolved when the next instruction
  is emitted.
- `patchlist(list, target)` resolves all jumps in the list to an
  absolute target address.

This mechanism handles if/elseif/else chains, while/repeat loops,
and boolean short-circuit evaluation without requiring a second pass.

**If/elseif/else pattern**:

1. Each condition emits a conditional jump (the "false-exit" list).
2. At the end of each then-block, an unconditional JMP is added to
   the "escape" list (jumps to after the entire if-statement).
3. Each false-exit list is patched to the start of the next
   elseif/else clause.
4. The escape list is patched to the instruction after `end`.

**While loop pattern**:

1. Save label at condition start.
2. Condition emits false-exit jump list.
3. Compile body.
4. Emit JMP back to condition start.
5. Patch false-exit to after the loop.

**Numeric for loop pattern**:

```text
  OP_FORPREP base, offset_to_FORLOOP
  <body>
  OP_FORLOOP base, offset_to_body_start
```

`FORPREP` subtracts step from index and jumps forward to `FORLOOP`.
`FORLOOP` adds step, compares with limit, and jumps back if in range.
Three hidden local variables (`(for index)`, `(for limit)`,
`(for step)`) occupy registers `base` through `base+2`; the user's
loop variable is at `base+3`.

**Generic for loop pattern**:

```text
  JMP to TFORLOOP
  <body>
  OP_TFORLOOP base, 0, nvars
  JMP to body start
```

Three hidden control variables (`(for generator)`, `(for state)`,
`(for control)`) at `base` through `base+2`. `TFORLOOP` calls the
generator function and assigns results to user variables. If the
first result is nil, the loop exits by skipping the back-jump.

#### Code Generation Per Statement Type

**Assignment**: `storevar()` dispatches on the target kind:

| Target | Instruction | Notes |
|--------|------------|-------|
| `VLOCAL` | (none -- direct register write) | `exp2reg` into the local's register |
| `VUPVAL` | `OP_SETUPVAL reg, idx` | Value in any register |
| `VGLOBAL` | `OP_SETGLOBAL reg, const` | Value in any register |
| `VINDEXED` | `OP_SETTABLE table, key_rk, val_rk` | Both key and value accept RK |

**Function calls**: the function occupies register `base`, arguments
are in `base+1, base+2, ...`. `OP_CALL base, B, C` where B = nargs+1
(0 for varargs), C = nresults+1 (0 for multi-return). After the call,
`free_reg = base+1` (one result). Call-as-statement sets C=1 (zero
results).

**Tail calls**: `return f(args)` where `f(args)` is the only return
expression rewrites `OP_CALL` to `OP_TAILCALL`.

**Return**: `OP_RETURN first, nret+1`. For multi-return tail
position, uses `LUA_MULTRET`. For a single return value, the value
can stay in any register.

**Local declarations**: variable names are registered but not
activated until after all right-hand expressions are evaluated. This
ensures `local a = a` reads the outer scope's `a`. `adjustlocalvars()`
sets `startpc` for debug info. `removevars()` sets `endpc` when
leaving a scope.

#### Upvalue Resolution

Variable lookup walks the `FuncState` chain via `singlevaraux()`:

1. Search locals in the current function (`searchvar`).
2. If not found, recurse into the parent function.
3. If found as a local in an outer function, mark it as captured
   (`markupval`) so `OP_CLOSE` is emitted when its block exits.
4. Register it as an upvalue in each intermediate function via
   `indexupvalue()`.

Each upvalue descriptor records:

- `in_stack = true, index = register` for locals captured from the
  immediately enclosing function.
- `in_stack = false, index = upvalue_index` for upvalues chained
  through an intermediate function.

Duplicate detection: if the same variable is captured multiple times,
`indexupvalue()` reuses the existing slot (matching on `k` and
`info`).

#### CLOSURE Pseudo-Instructions

After emitting `OP_CLOSURE A, Bx` (where Bx indexes the child Proto
in the parent's protos array), the compiler emits one
pseudo-instruction per upvalue:

- `OP_MOVE 0, reg, 0` — capture local at `reg` as an open upvalue.
- `OP_GETUPVAL 0, idx, 0` — share upvalue `idx` from the parent
  closure.

These pseudo-instructions are not executed normally. The VM reads
them as data when executing `OP_CLOSURE` to initialize the new
closure's upvalue array.

#### Key Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `MAXSTACK` | 250 | Maximum registers per function |
| `MAXARG_Bx` | 262143 (2^18-1) | Maximum unsigned Bx field |
| `MAXARG_sBx` | 131071 (2^17-1) | Maximum signed sBx field |
| `MAXINDEXRK` | 255 | Maximum constant index in RK operand |
| `BITRK` | 256 | Bit flag distinguishing RK constants |
| `NO_JUMP` | -1 | Sentinel for empty jump list |
| `NO_REG` | 255 | Sentinel for "no register" |
| `LUAI_MAXVARS` | 200 | Maximum local variables per function |
| `LUAI_MAXUPVALUES` | 60 | Maximum upvalues per function |
| `LFIELDS_PER_FLUSH` | 50 | Table constructor batch size |

## Tradeoffs

### vs. Single-Pass (PUC-Rio)

| Aspect | Single-pass | AST-based |
|--------|------------|-----------|
| Memory | Lower (no AST allocation) | Higher (AST in memory) |
| Speed | Faster compilation | Slower (two passes) |
| Testability | Hard (phases coupled) | Each phase testable alone |
| Debuggability | Hard (interleaved logic) | Clear phase boundaries |
| Future optimization | Requires parser changes | Operates on AST |

For a Rust implementation prioritizing correctness and maintainability,
the AST approach is the right tradeoff. Compilation speed is not the
bottleneck — Lua programs are typically small.

### vs. Full Lossless AST (full-moon)

We do NOT need a lossless AST that preserves whitespace and comments.
That is a requirement for formatters and linters, not for a VM. Our
AST only needs to preserve semantics and source locations for error
reporting.
