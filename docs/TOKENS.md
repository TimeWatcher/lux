# Lexer and Tokens

This document defines the lexical layer for Lux MVP 0.1.

The goal is to keep tokenization simple, deterministic, and practical for a
Rust implementation.

## 1. Lexer Goals

- support Lux syntax without requiring in-game parsing
- preserve Lua familiarity where practical
- prefer longest-match tokenization
- keep `?`-family operators unambiguous
- make brace-based parsing straightforward
- leave host-specific interpretation to later stages

## 2. General Rules

### Longest match wins

When multiple tokens share a prefix, the lexer must prefer the longest valid
token.

Examples:

- `...` over `..`
- `..=` over `..`
- `=>` over `=`
- `->` over `-`
- `??` over `?`
- `?.` over `?`
- `?:` over bare `?`

### Whitespace

Spaces, tabs, carriage returns, and newlines are normally trivia.

Lux MVP 0.1 is **not** indentation-sensitive, so the lexer does not emit
INDENT/DEDENT tokens.

### Comments

Lux should preserve Lua-style comments:

```lua
-- line comment
--[[
  block comment
]]
```

Comments are trivia and normally not emitted as semantic tokens, but source
locations should preserve them for diagnostics and formatting tooling later.

## 3. Token Categories

### 3.1 Identifiers

```text
Identifier
```

General shape:

- starts with `_` or an ASCII letter
- continues with `_`, ASCII letters, or digits

MVP 0.1 may stay ASCII-first for simplicity, with future Unicode support as a
separate decision.

Examples:

- `x`
- `_temp`
- `player`
- `RoundedBox`

### 3.2 Keywords

Lux-specific keywords in MVP 0.1:

- `fn`
- `if`
- `then`
- `else`
- `local`
- `const`
- `nil`
- `true`
- `false`
- `and`
- `or`
- `not`
- `import`
- `export`

Lua-compatible statement keywords used by MVP control flow include:

- `do`
- `while`
- `for`
- `repeat`
- `until`
- `break`
- `return`
- `in`

Classic Lua surface keywords such as `function` and `end` should still be
tokenized as reserved words for compatibility, diagnostics, and future
interop decisions.

Even though Lux style prefers braces and `fn`, the lexer should not pretend the
Lua keyword set does not exist.

### 3.3 Literals

```text
NumberLiteral
StringLiteral
TemplateStringStart
TemplateStringText
TemplateExprStart
TemplateExprEnd
TemplateStringEnd
NilLiteral
BooleanLiteral
```

#### Numbers

MVP 0.1 should support Lua-like number literals first.

At minimum:

- integer decimal: `123`
- float decimal: `12.5`
- exponent form: `1e3`, `2.5e-2`
- hex literals may be supported if aligned with Lua compatibility goals

#### Strings

Standard Lua-like quoted strings:

- `"hello"`
- `'hello'`

Long bracket strings may remain a later parser concern if needed for
compatibility.

#### Template strings

Lux adds backtick-delimited template strings:

```lux
`Count: ${count()}`
```

The lexer should treat template strings as a mode-switching construct rather
than as plain string literals.

Recommended token model:

- `TemplateStringStart`
- one or more `TemplateStringText`
- `TemplateExprStart` for `${`
- normal expression tokens inside the interpolation
- `TemplateExprEnd` for `}`
- `TemplateStringEnd`

This avoids forcing the parser to re-scan embedded source manually.

## 4. Punctuation and Delimiters

### Core delimiters

- `(`
- `)`
- `{`
- `}`
- `[`
- `]`
- `,`
- `;`

Notes:

- `;` remains optional and should be accepted as a separator.
- `{}` are especially important because Lux MVP 0.1 uses brace blocks heavily.

### Access punctuation

- `.`
- `:`

### Optional-chain punctuation

- `?.`
- `?:`

These must be single tokens, not post-processed punctuation pairs.

### Null coalescing

- `??`

## 5. Operators

### Assignment operators

- `=`
- `+=`
- `-=`
- `*=`
- `/=`
- `%=`
- `^=`
- `..=`

### Arithmetic operators

- `+`
- `-`
- `*`
- `/`
- `%`
- `^`

### Concatenation

- `..`

### Comparison operators

- `==`
- `~=`
- `<`
- `<=`
- `>`
- `>=`

### Arrow/function introducers

- `=>`
- `->`

These are lexical tokens, but they are only valid in function-expression
contexts at parse time.

### Other symbols

- `#`

## 6. Special Multi-Character Cases

This section covers the tricky groups that matter most for implementation.

### Dot family

Possible tokens:

- `.`
- `..`
- `...`
- `..=`

Lexer rule:

1. if `..=` then emit `DotDotEq`
2. else if `...` then emit `Ellipsis`
3. else if `..` then emit `DotDot`
4. else `.` emits `Dot`

This matters because Lux still wants Lua compatibility for varargs and
concatenation while also adding `..=`.

### Question family

Possible tokens:

- `?.`
- `?:`
- `??`
- bare `?` is not a standalone MVP token

Lexer rule:

1. if `?.` then emit `QuestionDot`
2. else if `?:` then emit `QuestionColon`
3. else if `??` then emit `QuestionQuestion`
4. otherwise bare `?` is a lexical error in MVP 0.1

This keeps the language strict and avoids half-supported syntax.

### Colon family

Possible tokens:

- `:`

Lexer rule:

1. `:` emits `Colon`

Safe method calls use `?:`, which belongs to the question family because its
longest-match prefix is `?`.

### Minus family

Possible tokens:

- `->`
- `-`
- `-=`

Lexer rule:

1. if `->` then emit `ArrowImplicitSelf`
2. else if `-=` then emit `MinusEq`
3. else emit `Minus`

### Equals family

Possible tokens:

- `=>`
- `==`
- `=`

Lexer rule:

1. if `=>` then emit `ArrowNormal`
2. else if `==` then emit `EqEq`
3. else emit `Eq`

## 7. Calls and Tail Syntax

The lexer does **not** emit special UI tokens for:

```lux
Label { text = "ok" }
Foo { a = 1 } { b = 2 }
```

These remain ordinary identifiers, braces, tables, and call-related tokens.

Any DSL-specific meaning is a parser / lowering concern, not a lexical one.

Likewise, the lexer does not distinguish block braces from table braces. That
decision belongs to the parser and depends on syntactic position.

## 8. Suggested Token Enum Shape

One possible Rust-facing token set:

```text
Identifier

KwFn
KwIf
KwThen
KwElse
KwLocal
KwNil
KwTrue
KwFalse
KwAnd
KwOr
KwNot
KwImport
KwExport
KwFunction
KwEnd
KwDo
KwWhile
KwFor
KwRepeat
KwUntil
KwBreak
KwReturn
KwIn

NumberLiteral
StringLiteral

TemplateStringStart
TemplateStringText
TemplateExprStart
TemplateExprEnd
TemplateStringEnd

LParen
RParen
LBrace
RBrace
LBracket
RBracket
Comma
Semicolon

Dot
DotDot
Ellipsis
Colon
QuestionDot
QuestionColon
QuestionQuestion

Eq
PlusEq
MinusEq
StarEq
SlashEq
PercentEq
CaretEq
DotDotEq

Plus
Minus
Star
Slash
Percent
Caret
Hash

EqEq
NotEq
Lt
LtEq
Gt
GtEq

ArrowNormal
ArrowImplicitSelf

Eof
```

The final enum may be renamed, but the surface distinctions above are useful.

## 9. Trivia and Source Spans

Even if comments and whitespace are skipped from semantic parsing, every token
should carry a source span.

Recommended minimum:

- byte start
- byte end
- line start
- line end

This will make diagnostics and future formatting support much easier.

## 10. Explicit Non-Goals for MVP 0.1 Lexer

- indentation tokenization
- Unicode identifier complexity
- macro-token expansion
- host-specific tokens
- game-runtime lexing
