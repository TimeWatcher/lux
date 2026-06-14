# GLua Backend Draft

This document records MVP 0.1 backend expectations for generating GLua-friendly
Lua from Lux source.

It exists to keep target-specific lowering details out of the core language
spec while still making the engineering contract explicit.

## 1. Backend Goals

- compile offline in `luxc`
- emit plain Lua 5.1 / GLua-compatible code
- avoid double evaluation of effectful expressions
- keep generated helpers small and predictable
- preserve enough source correlation for real debugging in GMod

## 2. Module Graph and Loading

Lux `import` / `export` is resolved by the compiler, not by the runtime parser.

Example source:

```lux
import { arr } from "lux/std"
import "lux/ui/register_defaults"
export fn positives(xs) = arr.filter(xs, (x) => x > 0)
```

MVP backend direction:

1. `luxc` resolves the module graph offline
2. each compiled module emits a Lua chunk with explicit exports
3. backend-generated loader glue decides how GLua loads that chunk

Side-effect-only imports participate in graph resolution too, but they bind no
local symbols in the importing module.

Macro imports are compile-time only:

```lux
import macro { dbg } from "lux/macros"
import macro * as gmodMacros from "lux/gmod/macros"
```

They do not become GLua imports and do not participate in runtime loader output.

That loader glue must stay aware of GMod execution environments:

- shared
- client
- server

The exact file layout can evolve, but the backend must not pretend GMod module
loading is the same as stock Lua `require`.

Host transforms may consume specific import specifiers and inject narrower
runtime imports. For example, a package such as `lux/ui` can declare a host
contract whose `target` and `runtime` are both `lux/ui`; component calls can
then fold to a `node` runtime import instead of importing every original
component binding. Runtime external artifacts are therefore computed from
transformed IR rather than from the raw source import graph.

## 3. Export Shape

### Default `fn` binding model

The default meaning of a simple-name `fn` declaration is a lexical binding in
the current scope.

At module top level, that means:

- private to the module
- not a GLua global
- not automatically exported
- not attached to an implicit module table

Nested simple-name `fn` declarations behave the same way within their enclosing
lexical scope.

### Exported lexical bindings

`export fn foo(...)` means:

1. create the same lexical binding as plain `fn foo(...)`
2. register that binding in the module export surface

Example:

```lux
fn helper(x) = x + 1

export fn publicApi(x) =
  helper(x) * 2
```

One acceptable backend shape is:

```lua
local __lux_exports = {}

local helper
local publicApi

helper = function(x)
  return x + 1
end

publicApi = function(x)
  return helper(x) * 2
end

__lux_exports.publicApi = publicApi

return __lux_exports
```

### Hoisting rule for simple-name `fn`

Within a lexical scope, simple-name function declarations should hoist their
bindings so forward references and mutual recursion work.

That means:

```lux
fn a() = b()
fn b() = 1
```

may lower like:

```lua
local a
local b

a = function()
  return b()
end

b = function()
  return 1
end
```

This rule applies to `fn` declarations only. It does not apply to arbitrary
`local` initializers.

### Host table declarations

GLua-oriented dotted and method declarations are different:

```lux
fn PANEL:Paint(w, h) {
  drawBody(self, w, h);
}

fn SWEP.PrimaryAttack() {
  fire(self);
}
```

These are side-effecting declarations on existing tables/receivers, not module
lexical exports.

Intent:

```lua
function PANEL:Paint(w, h)
  drawBody(self, w, h)
end

function SWEP.PrimaryAttack()
  fire(self)
end
```

In MVP 0.1, `export fn` is reserved for simple lexical names rather than
dotted/method host declarations.

One acceptable lowering strategy is a generated export table:

```lua
local __lux_exports = {}

local function positives(xs)
  return arr.filter(xs, function(x)
    return x > 0
  end)
end

__lux_exports.positives = positives

return __lux_exports
```

This is not the only valid representation, but the backend must provide a
stable module boundary so offline imports can be linked predictably.

## 4. Optional Access and Call Lowering

### Safe field access

```lux
player?.name
```

should evaluate `player` once and produce `nil` when the receiver is `nil`.

Intent:

```lua
local __lux_obj = player
local __lux_val = nil

if __lux_obj ~= nil then
  __lux_val = __lux_obj.name
end
```

### Safe indexed access

```lux
tbl?.[key]
```

Intent:

```lua
local __lux_obj = tbl
local __lux_val = nil

if __lux_obj ~= nil then
  local __lux_key = key
  __lux_val = __lux_obj[__lux_key]
end
```

The key expression must not run before the receiver is known to be non-`nil`.
For `tbl?.[sideEffect()]`, `sideEffect()` runs only inside the guarded branch.

### Safe dot call

```lux
mgfx?.RoundedBox(...)
```

should fetch the field once and only call it when the function value exists.

Intent:

```lua
local __lux_obj = mgfx
local __lux_fn = nil

if __lux_obj ~= nil then
  __lux_fn = __lux_obj.RoundedBox
end

if __lux_fn ~= nil then
  __lux_fn(...)
end
```

### Safe colon call

```lux
player?:GetName()
```

should fetch the method once and pass the receiver exactly once.

Intent:

```lua
local __lux_obj = player
local __lux_method = nil

if __lux_obj ~= nil then
  __lux_method = __lux_obj.GetName
end

if __lux_method ~= nil then
  __lux_method(__lux_obj)
end
```

### Direct-segment safety only

Optional safety only protects the segment that is explicitly marked.

```lux
factory?.Make()()
```

Intent:

```lua
local __lux_obj = factory
local __lux_fn = nil
local __lux_result = nil

if __lux_obj ~= nil then
  __lux_fn = __lux_obj.Make
end

if __lux_fn ~= nil then
  __lux_result = __lux_fn()
end

__lux_result()
```

If `factory?.Make()` yields `nil`, the final plain call still errors normally.

## 5. Null Coalescing

`??` must only treat `nil` as missing.

```lux
player?:GetName() ?? "Unknown"
```

cannot lower through Lua `and/or`, because `false` is a valid retained value.

Intent:

```lua
local __lux_tmp = __lux_val
local name

if __lux_tmp ~= nil then
  name = __lux_tmp
else
  name = "Unknown"
end
```

## 6. Conditional Lowering by Context

The same Lux conditional expression needs different Lua output depending on how
it is used.

### Statement position

```lux
isAdmin then Approve() else Decline()
```

Intent:

```lua
if isAdmin then
  Approve()
else
  Decline()
end
```

### Value position

```lux
local result = isAdmin then Approve() else Decline()
```

Intent:

```lua
local result
if isAdmin then
  result = Approve()
else
  result = Decline()
end
```

### Return position

```lux
fn choose(isAdmin) =
  isAdmin then Approve() else Decline()
```

Intent:

```lua
local function choose(isAdmin)
  if isAdmin then
    return Approve()
  else
    return Decline()
  end
end
```

Normalized IR should therefore route conditional emission through explicit
statement, value, and return contexts rather than through a single generic
temporary-based rewrite.

## 7. Nil-Safe Comparisons

This pattern is expected to be common in GLua:

```lux
if player?:GetExp() > 5 {
  award()
}
```

For relational operators, an optional-chain operand that resolves to `nil`
should make the comparison result `false`.

Intent:

```lua
local __lux_obj = player
local __lux_left = nil

if __lux_obj ~= nil then
  local __lux_method = __lux_obj.GetExp
  if __lux_method ~= nil then
    __lux_left = __lux_method(__lux_obj)
  end
end

if __lux_left ~= nil and __lux_left > 5 then
  award()
end
```

## 8. Lua Multivalue and Varargs

Because Lux targets GLua/Lua 5.1 compatibility, backend lowering must preserve
multivalue behavior in Lua-sensitive positions.

Examples:

```lux
fn passthrough() = f()
fn logAll(...) {
  return ...
}
local a, b = f()
call(prefix, f())
local xs = { 1, f() }
```

Required backend behavior:

- `return f()` must preserve all returned values
- `return ...` must preserve all varargs
- the final slot of assignment/local declaration value lists may preserve
  multivalue
- the final array field of a table literal may preserve Lua table-constructor
  multivalue expansion
- the final call argument may preserve multivalue

Lowering must not accidentally collapse these cases through unnecessary
temporaries.

## 9. Compound Assignment

Compound assignment must preserve single-evaluation semantics for complex
targets.

```lux
getTable()[nextIndex()] += 1
getPlayer().score += 1
```

Intent:

```lua
local __lux_tbl = getTable()
local __lux_key = nextIndex()
__lux_tbl[__lux_key] = __lux_tbl[__lux_key] + 1

local __lux_obj = getPlayer()
__lux_obj.score = __lux_obj.score + 1
```

This is easiest when normalization first lowers syntax targets into a
single-evaluation place abstraction.

## 10. Statement-Position Expression Lowering

Expression statements should lower according to intent:

- plain calls stay plain calls
- conditional expressions in statement position lower to `if`
- other expressions may use a discard temporary if necessary

Example:

```lux
isAdmin then Approve() else Decline()
```

Intent:

```lua
if isAdmin then
  Approve()
else
  Decline()
end
```

## 11. Readable Lua Output

Generated Lua should be inspectable during GMod debugging. The backend therefore
uses a readable pretty-printer rather than preserving every conservative
parenthesis from lowering:

- binary and unary expressions are emitted with Lua precedence, so
  `return x + 1` does not become `return (x + 1)`
- required parentheses are still preserved, for example
  `return (a + b) * (c - d)`
- empty tables emit as `{}`
- long table constructors and function calls wrap structurally instead of
  producing one very long line

This is a codegen readability policy only. It must not change evaluation order,
multi-return handling, nil-safe lowering, or short-circuit value semantics.

## 12. Source Correlation

Backend output must stay debuggable inside GMod.

At minimum, the generated Lua should preserve useful source correlation through
one or more of:

- stable line-preserving code generation where practical
- optional emitted source comments such as `--#lux source: src/foo.lux:42`
- sidecar source maps for richer tooling and production builds

Lux supports four inline source-comment modes:

- `none`: emit no inline comments; rely on sidecar source maps
- `readable`: emit comments only at review anchors such as functions and branch blocks
- `boundary`: emit comments when the mapped source line changes
- `dense`: emit comments for every mapped generated line

The CLI can emit inline source comments for inspectable generated examples:

```powershell
cargo run -- compile examples\features.lux --source-comments readable
cargo run -- compile examples\features.lux --source-comments boundary
cargo run -- compile examples\features.lux --source-comments dense
```

Temporary names should stay predictable, for example `__lux_obj_1` or
`__lux_tmp_2`, so generated stack traces and console dumps remain readable.

Hoisted lexical function predeclarations should map to the original `fn`
declaration line, not to the module start. If the mapping is synthetic, it
should still carry the source span that caused the generated line.

## 13. Gensym Hygiene

Predictable temp names are helpful, but they must still be collision-safe.

The backend should therefore allocate synthetic locals through a gensym system
that:

- uses a reserved Lux prefix
- keeps a module-local counter
- is aware of already-bound user locals and parameters

This avoids accidental clashes with user code such as `local __lux_obj = ...`.

## 14. Backend Boundary

The GLua backend is still language-agnostic with respect to hosts.

That means:

- no UI-only assumptions in core lowering
- no automatic `children` semantics unless a host transform requested it
- no VGUI-specific codegen unless the active host plugin opts in

Host transforms should run on resolved bindings, not raw identifier names. For
example, a `Column` binding imported from `lux/ui` may be transformed, while a
user-local `Column` function with the same spelling must not be.
