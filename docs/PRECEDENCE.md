# Precedence and Parsing

This document defines Lux MVP 0.1 operator precedence, associativity, and a few
important parsing rules for the new syntax.

## 1. Notes

- Lux keeps Lua-like precedence where practical.
- New syntax should prefer explicit semantics over clever ambiguity.
- `=>` and `->` are function introducers, not ordinary binary operators.
- `if {}` expressions and `then/else` forms are both conditional expressions,
  but they use different surface syntax.

## 2. Precedence Table

From lowest precedence to highest precedence:

| Level | Forms | Associativity | Notes |
|---|---|---|---|
| 1 | assignment, compound assignment | statement-only | `=`, `+=`, `..=` and friends are not expressions |
| 2 | conditional expressions | right | `a then b else c`, `if a { b } else { c }` |
| 3 | pipeline | left | `value |> f(%)`; `%` marks the insertion point in the immediate RHS expression |
| 4 | logical `or` | left | Lua-like boolean `or` |
| 5 | logical `and` | left | Lua-like boolean `and` |
| 6 | comparisons | non-associative | `==`, `~=`, `<`, `<=`, `>`, `>=` |
| 7 | null coalescing | right | `??` |
| 8 | concatenation | right | `..` |
| 9 | additive | left | `+`, `-` |
| 10 | multiplicative | left | `*`, `/`, `%` |
| 11 | unary | right | `not`, `#`, unary `-` |
| 12 | power | right | `^` |
| 13 | postfix chain | left | `.`, `?.`, `?:`, indexing, calls, tail table calls |
| 14 | primary | n/a | names, literals, tables, parenthesized expressions, `%` in the immediate pipeline RHS |

## 3. Conditional Expressions

Lux supports two surface forms:

```lux
cond then yes else no
if cond { yes } else { no }
```

Both lower to the same normalized conditional form.

### Associativity

Conditionals are right-associative:

```lux
a then b else c then d else e
```

parses as:

```lux
a then b else (c then d else e)
```

## 4. Null Coalescing

`??` is a value-level fallback operator.

Examples:

```lux
a ?? b + c
(player?:GetExp() ?? 0) > 5
player?:GetName() ?? "Unknown"
```

### Required parentheses with comparisons

Lux does not allow bare `??` and comparison operators in the same expression.
The grammar has an internal precedence order, but the parser still rejects this
surface form because the intended grouping is too easy to misread:

```lux
player?:GetExp() ?? 0 > 5
```

Write the nil-fallback grouping explicitly:

```lux
(player?:GetExp() ?? 0) > 5
```

Or, if the comparison is the fallback value, write that grouping explicitly:

```lux
player?:GetExp() ?? (0 > 5)
```

The same rule applies to equality and relational comparisons:

```lux
a ?? b == c  -- invalid
(a ?? b) == c
a ?? (b == c)
```

## 5. Optional Chaining

Lux supports:

```lux
obj?.field
obj?.[key]
obj?.field(args)
obj?:method(args)
```

Optional operators belong to the postfix chain level.

### Direct-segment safety model

Optional safety applies to the directly marked access/call segment only.

Example:

```lux
mgfx?.RoundedBox(...)
```

is treated as a safe dot call.

But:

```lux
factory?.Make()()
```

parses as a safe dot call followed by a normal call on the result.

If `factory?.Make()` yields `nil`, the trailing `()` is not automatically
protected and follows normal Lua error behavior.

### Method form

`?:` is its own postfix chain segment:

```lux
player?:GetExp()
```

It is not parsed as separate `:` and `?` tokens at the AST level.

### Dot call vs colon call

These are distinct:

```lux
obj?.fn(a)
obj?:fn(a)
```

The first is a safe dot call. The second is a safe colon call.

Safe indexed access is also supported:

```lux
tbl?.[key]
```

This is an optional index segment, not a special call form.
The key expression is evaluated only if the receiver is non-`nil`.

The following must stay distinct:

```lux
obj?.name(args)
(obj?.name)(args)
tbl?.[key](args)
```

They parse as:

- a safe dot call
- an optional member access followed by a normal outer call
- an optional indexed access followed by a normal call

## 6. Calls and Tail Syntax

Lux preserves Lua tail table calls:

```lux
Label { text = "ok" }
```

and callable chaining:

```lux
Foo { a = 1 } { b = 2 }
```

which parses as:

```lux
(Foo { a = 1 }) { b = 2 }
```

This is general language behavior, not UI-specific syntax.

Host plugins may later fold recognized chained tail blocks into domain-specific
fields such as `children`.

Core Lux itself does not assign `children` meaning to a second brace block.

## 7. Arrow Functions

`=>` and `->` are parsed in function-expression contexts only.

Examples:

```lux
(a) => a + 1
(w, h) -> self:PaintBody(w, h)
```

They are not general-purpose operators and are therefore excluded from the
binary precedence ladder.

### `->`

`->` introduces an implicit `self` parameter.

```lux
panel.Paint = (w, h) -> draw(self, w, h)
```

lowers to:

```lua
panel.Paint = function(self, w, h)
  return draw(self, w, h)
end
```

## 8. Compound Assignment

Compound assignment is statement-only:

```lux
x += 1
name ..= "x"
tbl[i] *= 2
```

It is not valid inside an expression position.

Targets must be assignable and may not include optional chain segments.

Examples:

```lux
player.score += 5        -- valid
player?:GetScore() += 5  -- invalid
```

## 9. Pipeline

Pipeline uses an explicit placeholder.

```lux
value |> f(%)
value |> clamp(0, %, 100)
xs |> arr.filter(%, (x) => x ~= nil)
```

The compiler does not implicitly insert the piped value as the first argument.
The right-hand side of `|>` must contain `%` outside nested function bodies.

`%` remains the modulo operator in infix position:

```lux
a % b
```

## 10. Brace Context Rule

`{}` meaning is determined by syntactic position, not by its contents.

Rules:

- in control-structure and function-body positions, `{}` is a block
- after an expression in tail-call position, `{}` is a tail table call argument
- in general expression position, `{}` is a table literal

Examples:

```lux
if ok { x } else { y }    -- blocks
Label { text = "ok" }     -- tail table call
local t = { x = 1 }       -- table literal
```

## 11. Recommended Parentheses Cases

Even with fixed precedence, these are good style:

```lux
(player?:GetExp() ?? 0) > 5
(if cond { a } else { b }) + c
```

## 12. Worked Parse Examples

These examples pin down several tricky cases:

```lux
a ?? b + c
```

parses as:

```lux
a ?? (b + c)
```

```lux
a + b ?? c
```

parses as:

```lux
(a + b) ?? c
```

```lux
cond then a else b ?? c
```

parses as:

```lux
cond then a else (b ?? c)
```

```lux
player?:GetExp() ?? 0 > 5
```

is rejected. Use:

```lux
(player?:GetExp() ?? 0) > 5
```

```lux
x => x + 1
```

parses as a function expression whose body is:

```lux
x + 1
```

```lux
Foo { a = 1 }.bar
```

parses as:

```lux
(Foo { a = 1 }).bar
```

```lux
Foo { a = 1 } { b = 2 }.baz
```

parses as:

```lux
((Foo { a = 1 }) { b = 2 }).baz
```

```lux
x |> clamp(0, %, 100)
```

parses as a pipeline whose RHS contains a placeholder. It lowers as though the
left side is evaluated once and substituted at `%`.

The placeholder scope is deliberately shallow. `%` is valid only in the
immediate right-hand expression layer of the current `|>` expression. It does
not propagate into nested function or arrow bodies:

```lux
xs |> arr.map(%, (x) => x + %)
```

is rejected. Write the captured pipeline value explicitly before entering a
lambda if that is the intended behavior.
