# Instruction Set

## Decision

**PUC-Rio's 38 register-based opcodes, represented as Rust enums
in-memory, packed as u32 for bytecode serialization.**

## PUC-Rio Lua 5.1.1 Opcodes

The 38 opcodes from `lopcodes.h`, grouped by function:

### Loading and Moving

| Opcode | Format | Description |
|--------|--------|-------------|
| MOVE | iABC | `R(A) := R(B)` |
| LOADK | iABx | `R(A) := Kst(Bx)` |
| LOADBOOL | iABC | `R(A) := (Bool)B; if (C) pc++` |
| LOADNIL | iABC | `R(A) := ... := R(B) := nil` |

### Upvalue and Global Access

| Opcode | Format | Description |
|--------|--------|-------------|
| GETUPVAL | iABC | `R(A) := UpValue[B]` |
| GETGLOBAL | iABx | `R(A) := Gbl[Kst(Bx)]` |
| GETTABLE | iABC | `R(A) := R(B)[RK(C)]` |
| SETGLOBAL | iABx | `Gbl[Kst(Bx)] := R(A)` |
| SETUPVAL | iABC | `UpValue[B] := R(A)` |
| SETTABLE | iABC | `R(A)[RK(B)] := RK(C)` |

### Table Creation

| Opcode | Format | Description |
|--------|--------|-------------|
| NEWTABLE | iABC | `R(A) := {} (size = B, C)` |

### Arithmetic and Logic

| Opcode | Format | Description |
|--------|--------|-------------|
| SELF | iABC | `R(A+1) := R(B); R(A) := R(B)[RK(C)]` |
| ADD | iABC | `R(A) := RK(B) + RK(C)` |
| SUB | iABC | `R(A) := RK(B) - RK(C)` |
| MUL | iABC | `R(A) := RK(B) * RK(C)` |
| DIV | iABC | `R(A) := RK(B) / RK(C)` |
| MOD | iABC | `R(A) := RK(B) % RK(C)` |
| POW | iABC | `R(A) := RK(B) ^ RK(C)` |
| UNM | iABC | `R(A) := -R(B)` |
| NOT | iABC | `R(A) := not R(B)` |
| LEN | iABC | `R(A) := length of R(B)` |
| CONCAT | iABC | `R(A) := R(B).. ... ..R(C)` |

### Jumps and Comparisons

| Opcode | Format | Description |
|--------|--------|-------------|
| JMP | iAsBx | `pc += sBx` |
| EQ | iABC | `if ((RK(B) == RK(C)) ~= A) then pc++` |
| LT | iABC | `if ((RK(B) < RK(C)) ~= A) then pc++` |
| LE | iABC | `if ((RK(B) <= RK(C)) ~= A) then pc++` |
| TEST | iABC | `if not (R(A) <=> C) then pc++` |
| TESTSET | iABC | `if (R(B) <=> C) then R(A) := R(B) else pc++` |

### Function Calls

| Opcode | Format | Description |
|--------|--------|-------------|
| CALL | iABC | `R(A), ... ,R(A+C-2) := R(A)(R(A+1), ... ,R(A+B-1))` |
| TAILCALL | iABC | `return R(A)(R(A+1), ... ,R(A+B-1))` |
| RETURN | iABC | `return R(A), ... ,R(A+B-2)` |

### Loops

| Opcode | Format | Description |
|--------|--------|-------------|
| FORLOOP | iAsBx | Numeric for loop step |
| FORPREP | iAsBx | Numeric for loop init |
| TFORLOOP | iABC | Generic for loop step |

### Tables and Closures

| Opcode | Format | Description |
|--------|--------|-------------|
| SETLIST | iABC | `R(A)[(C-1)*FPF+i] := R(A+i), 1 <= i <= B` |
| CLOSE | iABC | Close upvalues >= R(A) |
| CLOSURE | iABx | `R(A) := closure(KPROTO[Bx], R(A), ... ,R(A+n))` |
| VARARG | iABC | `R(A), R(A+1), ..., R(A+B-1) = vararg` |

## Rust Representation

### In-Memory: Typed Enum

```rust
/// A single VM instruction.
///
/// Operands use the same semantics as PUC-Rio Lua 5.1.1:
/// - A: 8-bit register index (0-255)
/// - B, C: 9-bit register/constant index (0-511, bit 8 = constant)
/// - Bx: 18-bit unsigned index
/// - sBx: 18-bit signed offset (excess-K encoding)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    Move { a: u8, b: u16 },
    LoadK { a: u8, bx: u32 },
    LoadBool { a: u8, b: u16, c: u16 },
    LoadNil { a: u8, b: u16 },
    GetUpval { a: u8, b: u16 },
    GetGlobal { a: u8, bx: u32 },
    GetTable { a: u8, b: u16, c: u16 },
    SetGlobal { a: u8, bx: u32 },
    SetUpval { a: u8, b: u16 },
    SetTable { a: u8, b: u16, c: u16 },
    NewTable { a: u8, b: u16, c: u16 },
    SelfOp { a: u8, b: u16, c: u16 },
    Add { a: u8, b: u16, c: u16 },
    Sub { a: u8, b: u16, c: u16 },
    Mul { a: u8, b: u16, c: u16 },
    Div { a: u8, b: u16, c: u16 },
    Mod { a: u8, b: u16, c: u16 },
    Pow { a: u8, b: u16, c: u16 },
    Unm { a: u8, b: u16 },
    Not { a: u8, b: u16 },
    Len { a: u8, b: u16 },
    Concat { a: u8, b: u16, c: u16 },
    Jmp { sbx: i32 },
    Eq { a: u8, b: u16, c: u16 },
    Lt { a: u8, b: u16, c: u16 },
    Le { a: u8, b: u16, c: u16 },
    Test { a: u8, c: u16 },
    TestSet { a: u8, b: u16, c: u16 },
    Call { a: u8, b: u16, c: u16 },
    TailCall { a: u8, b: u16 },
    Return { a: u8, b: u16 },
    ForLoop { a: u8, sbx: i32 },
    ForPrep { a: u8, sbx: i32 },
    TForLoop { a: u8, c: u16 },
    SetList { a: u8, b: u16, c: u16 },
    Close { a: u8 },
    Closure { a: u8, bx: u32 },
    VarArg { a: u8, b: u16 },
}
```

### Serialized: Packed u32

For bytecode serialization (future), instructions pack into PUC-Rio's
u32 format:

```text
iABC:  [  B:9  ][  C:9  ][ A:8 ][ Op:6 ]
iABx:  [    Bx:18       ][ A:8 ][ Op:6 ]
iAsBx: [   sBx:18       ][ A:8 ][ Op:6 ]
```

Conversion functions translate between the Rust enum and u32.

## RK Encoding

Operands B and C in iABC instructions use RK (Register or Konstant)
encoding. Bit 8 indicates:

- 0: register index (0-255)
- 1: constant index (0-255, after masking bit 8)

This allows arithmetic instructions to reference constants directly
without a preceding LOADK instruction.

## Why Rust Enums

PUC-Rio stores instructions as raw u32 values and extracts fields
with bit manipulation macros. We use a Rust enum because:

1. **Type safety** — Pattern matching ensures all opcodes are handled.
   Adding an opcode causes compile errors at every unhandled match.
2. **Clarity** — `Instruction::Add { a, b, c }` is self-documenting.
   `GETARG_A(i)` is not.
3. **No performance cost** — The enum representation is the same size
   or smaller than u32 in practice. The compiler optimizes match
   dispatch to jump tables.
4. **Serialization is separate** — The u32 packed format is only
   needed for bytecode files, not for in-memory execution.
