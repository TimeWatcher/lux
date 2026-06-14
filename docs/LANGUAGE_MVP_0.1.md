# Lux Language MVP 0.1

## 1. Positioning

- Lux is a Lua superset for GLua-oriented development.
- Compilation happens offline in `luxc`, not inside the game.
- Output must remain pure GLua / Lua 5.1 compatible code.
- UI is optional. `lux/ui` is a host package, not the definition of the language.

## 2. Core Syntax

### Evaluation order

Lux defines source expression evaluation order as left-to-right for expression
lists that the compiler controls, including call arguments, table fields,
assignment/local initializer value lists, return value lists, and macro-expanded
IIFE shapes.

This is stricter than relying on unspecified Lua 5.1 edge cases. Macro and host
transform authors must preserve observable source argument order unless the
macro explicitly documents a different evaluation model.

### `fn` functions

```lux
fn sum(a, b) = a + b

fn abs(x) {
  if x >= 0 { x } else { -x }
}

fn logAll(...) {
  return ...
}
```

Default parameters are nil-triggered:

```lux
fn label(text = "Untitled") = text
```

The default expression is evaluated only when the corresponding argument is
`nil` or omitted. Passing `false` does not trigger the default. Defaults are
emitted at the top of the generated Lua function body, in parameter order.
This means later defaults may observe earlier parameters after their defaults
have been applied:

```lux
fn f(a = 1, b = a + 1) = b
```

Reverse references are allowed but evaluated in the same parameter-order model:

```lux
fn f(a = b, b = 1) = a
```

Here `a` is evaluated before `b` receives its default, so this should be treated
as ordinary lexical/runtime name lookup rather than a special dependency graph.
Prefer earlier-to-later default dependencies for readability.

Lux function declarations must also support GLua-style dotted and method names:

```lux
fn M.foo(a) {
  return a
}

fn PANEL:Paint(w, h) {
  drawBody(self, w, h);
}
```

Intent:

- `fn foo(a)` creates a lexical function binding in the current scope
- `fn M.foo(a)` lowers like `function M.foo(a) ... end`
- `fn PANEL:Paint(w, h)` lowers like `function PANEL:Paint(w, h) ... end`

Block-tail implicit return still applies to GLua method declarations. For GMod
callbacks such as `Paint`, `Think`, and mouse/key handlers, the recommended
style is to terminate the final side-effect expression with `;` unless the
callback intentionally returns a value. The linter reports this as `LINT004`.

### Function binding semantics

Simple-name `fn` declarations are lexical bindings.

Rules:

- top-level `fn foo(...)` creates a module-private lexical binding
- nested `fn foo(...)` creates a lexical binding in the current block/function scope
- simple-name `fn` does not create a GLua global
- simple-name `fn` does not auto-export
- simple-name `fn` does not attach itself to an implicit module table

Example:

```lux
fn helper(x) = x + 1

export fn publicApi(x) =
  helper(x) * 2
```

Semantic intent:

```lua
local function helper(x)
  return x + 1
end

local function publicApi(x)
  return helper(x) * 2
end

__lux_exports.publicApi = publicApi
```

This is the default module model for Lux MVP 0.1:

- top-level `fn` is private
- `export` is the only way to expose a binding outside the module

### Module Parts

A Lux module is a logical module, not necessarily one file. In project
compilation, all part files under the same module directory share one logical
module scope.

Rules:

- top-level declarations in any part create module-private bindings visible to
  all parts of the same module, subject to realm checks
- top-level imports are part-local bindings, not module-wide bindings
- simple top-level `fn` declarations are hoisted across the whole module
- top-level non-function locals are not value-hoisted; initialization follows
  deterministic part order, and use-before-initialization is an error
- exports map module-scope bindings to public API names
- export does not affect internal visibility
- duplicate module-scope binding names are errors in MVP 0.1, even if declared
  in different realms

Multi-part modules must have one entry part. The entry part basename is
`module`; realm prefixes are allowed, so these are all valid entries:

- `module.lux`
- `cl_module.lux`
- `sv_module.lux`
- `sh_module.lux`

A module with more than one part and no entry is a compile error. A module with
more than one entry is also a compile error. Single-file modules do not need an
entry file; the file itself is the module.

Part initialization order is stable and can be declared in source:

```lux
part order { "module", "cl_base", "cl_progress", "cl_rings", "cl_install" }
part before "cl_install"
part after "cl_base"
```

`part order { ... }` is the preferred form for complete module part
arrangement. It must be written in the entry part. It orders the listed parts
by their path relative to the module directory, without the `.lux` extension.
Unlisted parts keep deterministic path order around the declared constraints.
The entry part sorts before other parts by default, so a metadata-only
`module.lux` does not need runtime code.

`part before` and `part after` are auxiliary local constraints for small
adjustments. They order the current part relative to the named target part, but
they are not intended to replace a full `part order` list for large modules.

Invalid targets, duplicate targets in one order list, ambiguous short names,
misplaced complete order declarations, and ordering cycles are compile errors.

### Immutable bindings

`const` declares an immutable lexical binding:

```lux
const maxPlayers = 64
const { name, hp = 100 } = player
```

Rules:

- `const` requires an initializer
- `const` supports the same destructuring forms as `local`
- assigning or compound-assigning the binding itself is an error
- imports are also immutable bindings
- duplicate declarations in the same lexical scope are errors
- inner blocks may shadow outer bindings, including const bindings
- `const` is not a deep freeze; mutating fields of a table held by a const
  binding is allowed

Example:

```lux
const state = { count = 0 }
state.count += 1 -- allowed
state = {}       -- error
```

### Host table declarations

GLua host-style declarations remain first-class:

```lux
fn PANEL:Paint(w, h) {
  drawBody(self, w, h);
}

fn SWEP.PrimaryAttack() {
  fire(self);
}
```

These are not lexical bindings. They are assignments/declarations on an
existing table/object path.

Rules:

- `fn A.B(...)` assigns a function to table field `A.B`
- `fn A:B(...)` declares a method on receiver `A`
- these forms are side-effecting declarations, not module-private locals
- these forms are not implicit exports

To keep module API rules explicit, `export fn` in MVP 0.1 is only valid with a
simple name. `export fn A.B(...)` and `export fn A:B(...)` are invalid.

### Function declaration hoisting

Simple-name `fn` declarations are binding-hoisted within their lexical scope.

This means Lux hoists the binding, not the execution of surrounding statements.

Example:

```lux
fn a() = b()
fn b() = 1
```

Semantic intent:

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

This allows forward references, self-recursion, and mutual recursion without
changing the meaning of ordinary `local` assignments.

Only simple-name `fn` declarations get this treatment. Arbitrary `local`
initializers do not, and dotted/method declarations preserve normal execution
order.

### Brace blocks

`{}` replaces most `end`-driven blocks in Lux syntax.

### Brace disambiguation

Brace meaning is determined by syntactic position, not by contents.

Rules:

- in control-structure positions and function-body positions, `{}` is a block
- after an expression in call position, `{}` is a tail table call argument
- in ordinary expression position, `{}` is a table literal

Examples:

```lux
if x > 0 { x } else { -x }   -- block expressions
Label { text = "hello" }     -- tail table call
local x = { text = "hello" } -- table literal
Foo { a = 1 } { b = 2 }      -- chained callable tail calls
```

### Arrow functions

Two forms are supported:

```lux
(a) => a + 1
(w, h) -> if self.isActive { drawActive(self, w, h) }
```

- `=>` creates a normal function
- `->` creates a function with implicit `self` as the first parameter

Lowering example:

```lua
function(a) return a + 1 end
function(self, w, h) ... end
```

### Explicit `return`

Lux supports explicit `return` statements.

```lux
fn make(self) {
  return (x) -> self.foo + x
}
```

Explicit `return` always wins over implicit block-return behavior.

### Lua-compatible statements

Lux MVP keeps core Lua-style statements for ordinary business logic.

Examples:

```lux
local x = 1
local a, b = 1, 2
const limit = 100

x = x + 1

do {
  setup()
}

if cond {
  log("ok")
} else {
  log("bad")
}

while running {
  tick()
}

for i = 1, 10 {
  print(i)
}

for k, v in pairs(tbl) {
  print(k, v)
}

repeat {
  step()
} until done

while running {
  if shouldStop {
    break
  }
}
```

Lux prefers brace blocks, but ordinary GLua-oriented imperative control flow
remains part of the language.

MVP statement surface explicitly includes:

- `local`
- assignment
- `return`
- `break`
- `if` / `else`
- `while`
- numeric `for`
- generic `for ... in`
- `repeat { ... } until ...`
- `do { ... }`

## 3. Expressions

### Expression statements

Any expression can be used as a statement.

```lux
log(`count = ${count()}`)
isAdmin then Approve() else Decline()
```

Lowering should respect user intent:

- plain call expressions stay plain call statements
- conditional expressions used as statements lower to real `if`
- general value expressions may use a discard temporary

Examples:

```lux
log("x")
isAdmin then Approve() else Decline()
```

Expected lowering shape:

```lua
log("x")

if isAdmin then
  Approve()
else
  Decline()
end
```

### Implicit expression return

```lux
fn choose(isAdmin) =
  isAdmin then Approve() else Decline()
```

### Varargs and Lua multivalue compatibility

Lux must preserve Lua-compatible varargs and multivalue behavior in Lua-sensitive
positions.

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

In non-Lua-sensitive expression positions, call results still collapse to a
single value just as they do in Lua.

### Destructuring bindings

Lux supports destructuring in local and const binding position.

```lux
local { name, hp, armor = 0 } = player
local [x, y, z = 0] = point
const { id } = player
```

Object destructuring reads named fields. Array destructuring reads one-based Lua
array positions. Defaults are used only when the extracted value is `nil`.
`false` is a present value and does not trigger a default.

Destructuring evaluates the right-hand side once and stores it in a compiler
temporary before reading fields. Plain `local a, b = f()` remains the normal Lua
multivalue-compatible local declaration path.

Destructuring does not nil-protect the source. These follow ordinary Lua
field/index behavior and will fail at runtime if the source is `nil`:

```lux
local { name } = nil
local [x] = nil
```

Use `?? {}` or optional access when `nil` is acceptable:

```lux
local { name } = maybePlayer ?? {}
```

### Table spread

Table constructors support ordered spread fields:

```lux
local props = { ...base, text = "OK", visible = true }
```

Spread copies key/value pairs with `pairs`. Later fields override earlier spread
values. A `nil` spread source is ignored. Table spread does not copy metatables
or hidden state; if a metatable matters, copy or set it explicitly.
Because `pairs(nil)` would fail in Lua, the backend must guard spread sources
before iterating them.

Plain Lua multivalue table behavior remains unchanged for ordinary array
fields: only the final array field may expand multiple return values.

### Do expressions

`do { ... }` is available in expression position:

```lux
local next = do {
  local current = count ?? 0
  current + 1
}
```

Its value follows the normal block value rule. If the block has no tail
expression, the do expression value is `nil`. In statement position, `do { ... }`
remains a scoped statement.

Backends should lower value-position do expressions into scoped assignment when
possible rather than forcing an IIFE. This preserves the block semantics without
adding avoidable call overhead.

### Block value rule

A block used in expression position has a value.

Its value is the final expression statement.

If the final item is not an expression statement, the block value is `nil`.

If the final expression is terminated by a trailing semicolon, it is treated as
an expression statement, not a block tail. This suppresses implicit block value
and function return behavior.

A function body may use that tail expression as an implicit return when one
exists, but Lux does not synthesize `return nil` just because an ordinary
statement ends the block.

Example:

```lux
local y = if ok {
  log("ok")
  1
} else {
  log("bad")
  0
}
```

The two branch blocks have values `1` and `0`.

### Function-body implicit return rule

If control reaches the final expression statement of a function body naturally,
that expression becomes the implicit return value.

Explicit `return` statements still take priority.

If the body ends in a non-expression statement, control falls through exactly
as it would in Lua. The compiler should not insert a synthetic `return nil`
just because a block expression in some other context would have the value
`nil`.

Examples:

```lux
fn a() {
  1 + 2
}
```

lowers like:

```lua
local function a()
  return 1 + 2
end
```

But:

```lux
fn b() {
  local x = 1
  x += 2
}
```

does not implicitly return anything, because compound assignment is a statement,
not an expression.

Likewise:

```lux
fn c() {
  1 + 2;
}
```

does not implicitly return `3`, because the final expression was explicitly
terminated as a statement.

And:

```lux
fn f(x) {
  if x < 0 {
    return 0
  }

  x + 1
}
```

lowers like:

```lua
local function f(x)
  if x < 0 then
    return 0
  end

  return x + 1
end
```

### `if` expression

```lux
if isAdmin { Approve() } else { Decline() }
```

### `then/else`

```lux
isAdmin then Approve() else Decline()
```

This is a language-level conditional expression, not a Lua `and/or` trick.

`then/or` is intentionally **not** part of MVP 0.1. Although short, it creates
avoidable ambiguity because `or` already exists as a boolean operator. The
canonical short conditional form for MVP is:

```lux
cond then yes else no
```

### Pipeline

Pipeline uses an explicit placeholder:

```lux
xs
  |> arr.filter(%, (x) => x ~= nil)
  |> arr.map(%, (x, index) => x + index)
```

The left side is evaluated once and substituted wherever `%` appears in the
right side. The right side of `|>` must contain `%`; Lux does not implicitly
insert the value as the first argument. Outside the right side of a pipeline,
`%` remains the modulo operator in infix position.

The placeholder scope is shallow. `%` is valid only in the immediate
right-hand expression layer of the current pipeline. It does not propagate into
nested `fn`, `=>`, or `->` bodies:

```lux
xs |> arr.map(%, (x) => x + %)
```

is invalid. Bind the pipeline value before the lambda when nested access is
needed.

### Enums and match

Scalar enums are compile-time by default:

```lux
enum FillKind repr number {
  Solid = 0,
  Linear = 1
}

fn label(kind) =
  match kind {
    FillKind.Solid => "solid"
    FillKind.Linear => "linear"
  }
```

`repr number` and `repr string` emit zero runtime tables unless the enum is
explicitly declared `runtime`. Variant references lower directly to their
literal tags.

`repr existing` is a view over an existing table layout:

```lux
enum Fill repr existing(kind = "kind") {
  Solid(kind = FILL_SOLID, color: Color),
  Linear(kind = FILL_LINEAR, from: Color, to: Color)
}
```

Matching an existing-layout enum reads the tag field directly. It is not
nil-safe by default:

```lua
local __lux_match_1 = fill
local __lux_tag_2 = __lux_match_1.kind
```

This is intentional for hot paths. A plain `match fill` means `fill` is expected
to be a valid subject. Users should guard with `stopif fill == nil`, match a
separate optional expression, or add explicit fallback logic when nil is a
valid input. The compiler's exhaustiveness checks cover enum variants; they do
not silently insert runtime nil guards.

## 4. Safe Access and Null Handling

### Optional operators

```lux
mgfx?.RoundedBox(...)
player?.name
player?:GetName()
tbl?.[key]
```

- `?.` safe field access / safe dot call
- `?.[` safe indexed access
- `?:` safe colon method call

### Direct-segment safety rule

Optional safety applies to the directly marked access/call segment.

It does not automatically protect later unmarked operations.

Example:

```lux
factory?.Make()()
```

The first part is a safe dot call. The final `()` is a normal call.

If `factory?.Make()` yields `nil`, the trailing `()` still follows normal Lua
error behavior.

### Dot call vs colon call

These are distinct:

```lux
obj?.fn(a)
obj?:fn(a)
```

Intent:

- `obj?.fn(a)` behaves like `obj.fn(a)` with safe receiver/field lookup
- `obj?:fn(a)` behaves like `obj:fn(a)` with safe receiver/method lookup

The dot-call form does not inject `self`. The colon-call form does.

Safe indexed access is also part of MVP 0.1:

```lux
tbl?.[key]
```

It safely performs the indexed access only when the receiver is non-`nil`.
Like the rest of optional chaining, it protects only that directly marked
segment. So:

```lux
tbl?.[key](args)
```

means safe indexed lookup followed by a normal call on the result.

The index expression is evaluated only after the receiver is known to be
non-`nil`. Therefore `tbl?.[sideEffect()]` does not call `sideEffect()` when
`tbl` is `nil`.

Safe dot and colon calls also delay argument evaluation until both the receiver
and callable segment exist. MVP 0.1 does not include a standalone optional call
operator such as `callee?.(args)`.

Lowering should also avoid repeated field lookup. A safe dot call such as:

```lux
mgfx?.RoundedBox(...)
```

should behave like:

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

And a safe colon call such as:

```lux
player?:GetName()
```

should behave like:

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

### Null coalescing

```lux
player?:GetName() ?? "Unknown"
```

`??` only handles `nil`. It does **not** treat `false` as empty.

So:

```lux
false ?? "fallback"
```

must evaluate to `false`, not `"fallback"`.

This means `??` cannot lower to Lua `and/or`. It must lower through explicit
temporary storage and `nil` testing.

`??` may not be mixed with comparison operators without parentheses. These are
ambiguous and rejected:

```lux
player?:GetExp() ?? 0 > 5
a ?? b == c
```

Write the intended grouping explicitly:

```lux
(player?:GetExp() ?? 0) > 5
(a ?? b) == c
a ?? (b == c)
```

### Nil-safe comparison lowering

Patterns such as:

```lux
if player?:GetExp() > 5 { ... }
```

should lower to code that does not double-evaluate the receiver and does not
raise on `nil`.

For relational comparisons (`<`, `<=`, `>`, `>=`):

- if an optional-chain operand resolves to `nil`, the comparison result is
  `false`
- if both sides are optional chains and either side resolves to `nil`, the
  comparison result is `false`

For equality operators:

- `==` and `~=` keep normal Lua-compatible semantics once the optional value has
  been safely evaluated into a temporary

Example intent:

```lux
if player?:GetExp() > 5 { ... }
```

behaves like:

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
  ...
end
```

Examples:

```lux
player?:GetExp() > other?:GetExp()
player?:GetExp() == nil
player?:GetExp() ~= nil
```

The relational comparison is false if either safe method returns `nil`. The
equality checks compare the safely evaluated value against `nil`, so they are
the recommended way to test whether an optional chain produced a value.

For non-boolean arithmetic-style usage, the recommended pattern is:

```lux
(player?:GetExp() ?? 0) + 5
```

## 5. Mutations

Compound assignments are supported:

```lux
x += 5
y -= 2
name ..= "me"
tbl[i] += 1
```

Supported operators in MVP:

- `+=`
- `-=`
- `*=`
- `/=`
- `%=`
- `^=`
- `..=`

Complex left-hand sides must be evaluated exactly once.

Examples of intended semantics:

```lux
tbl[i] += 1
getPlayer().score += 1
```

must lower as though the object/key side is captured once before the write.

## 6. Calls and Chaining

### Lua-style tail table call omission

```lux
Label { text = `Count: ${count()}` }
```

### Callable chaining

```lux
Foo { a = 1 } { b = 2 }
```

This is not UI-specific syntax. It follows normal left-associated callable
semantics:

```lux
(Foo { a = 1 }) { b = 2 }
```

The core compiler must not silently treat a second block as `children` for
arbitrary callees. Such folding is only valid through an explicit host-level
transform for known DSL symbols.

In other words, this UI-looking shape:

```lux
Column { gap = 12 } {
  Label { text = "Hi" }
}
```

is still just chained table-call syntax in core Lux. The first call must return
a callable value unless a host transform rewrites it.

## 7. Imports and Module Resolution

Lux `import` is a compile-time feature, not a runtime language primitive.

Example:

```lux
import { arr } from "@lux/std"
import * as std from "@lux/std"
import "@lux/ui/register_defaults"
import macro { dbg } from "@lux/macros"
```

means:

- `luxc` resolves the module graph offline
- backend lowering decides the target-specific load strategy

For the GLua backend, this must remain explicit and target-aware. It cannot be
left vague because GMod uses `include`, `AddCSLuaFile`, and environment-specific
loading rules rather than a normal Lua-only module story.

So for MVP:

- `import` syntax is part of the language
- module resolution is compile-time
- runtime loading style is backend-defined
- GLua backend rules will be specified separately from the surface syntax

Named imports can be aliased with `as`, and namespace imports bind the target
module export table:

```lux
import { arr as array } from "@lux/std"
import * as std from "@lux/std"
```

Lux also supports side-effect imports:

```lux
import "some/module"
```

This form binds no local names. It exists for modules that register hooks,
classes, defaults, or other startup behavior.

Some runtime imports may also be transformable by host packages. For example,
`import { Column } from "@lux/ui"` authorizes a host transform whose package
contract declares `target = "lux/ui"`. A transform may consume the original
component specifier and inject a smaller runtime import such as `node` from the
host package's declared `runtime` module. Runtime artifact generation is based
on this final transformed IR, not just the raw source import graph.

### Macro imports

Macro imports are explicit and compile-time only:

```lux
import macro { dbg } from "@lux/macros"
import macro * as gmodMacros from "@lux/gmod/macros"
```

Macro bindings do not create runtime module edges. They can only be called as
macros during compilation and cannot be used as runtime values.

### Exports

Lux modules also support exports.

Examples:

```lux
export fn foo() = 1
export const answer = 42
export { foo, bar }
export { p_inv = player_inventory }
export { player_inventory as p_inv }
```

Export semantics are lexical-first:

- `export fn foo(...)` declares a normal lexical binding first, then exports it
- `export const foo = value` declares an immutable lexical binding first, then
  exports it
- `export { foo, bar }` exports existing lexical bindings
- `export { public_name = local_binding }` and
  `export { local_binding as public_name }` export an alias; importing code must
  use the public name
- export is the only way to expose Lux bindings outside the module

The exact backend lowering of export registration is target-defined, but export
semantics are part of the language and required for offline module graph
resolution.

Realm annotations can appear on declarations and blocks:

```lux
server fn grantItem(player) { ... }
client fn openPanel() { ... }

server {
  util.AddNetworkString("inventory")
}

server init {
  registerHooks()
}
```

In a shared part, unmarked top-level declarations default to shared. A
realm-marked declaration has that realm even when it appears in a shared file,
so realm-specific code can live near shared code without weakening checks.

External GMod symbols use a three-level availability model:

- Lux symbols: strict
- known GMod API or declared `extern`: strict
- unknown external: allowed with `REALM_UNKNOWN` warning by default in GMod
  project builds

Extern declarations make third-party globals strict:

```lux
extern server ThirdPartyAddon
extern client FancyHud.Open
extern shared SharedLibrary
```

Extern paths use longest-prefix matching. `extern server net.Start` takes
priority over `extern shared net`.

Compile-time packages have additional phase-qualified exports:

```lux
export macro fn dbg(ctx, call) { ... }
export host package {
  target = "lux/ui",
  runtime = "lux/ui"
}
export host expr fn foldNode(ctx, call) { ... }
```

These phase-qualified declarations are compile-time package declarations. In a
normal runtime module they are compile errors, not ignored syntax.

`export macro fn` exposes a syntax macro to `import macro`. `export host expr fn`
registers an expression host transform for the package. These declarations are
not runtime module exports and are rejected in normal runtime modules.

Every compile-time package that exports `export host expr fn` must declare
exactly one `export host package` contract:

- `target`: runtime source module whose imported symbols the host may transform
- `runtime`: runtime source module used by `ctx.importRuntime(...)`

The two modules may be the same, as with `lux/ui`, or different, as with a
project-specific host that rewrites `my/ui` calls into support imports from
`my/runtime`.

Host transform calls expose both import names:

- `call.imported`: the original exported name from `target`
- `call.local`: the local alias used in the importing source file

For `import { Column as Stack } from "lux/ui"`, a host transform sees
`call.imported == "Column"` and `call.local == "Stack"`.

`ctx.importRuntime(imported, preferredLocal)` returns the actual local binding
name used for the injected runtime import. Host transforms must use that return
value when constructing IR, because the compiler may rename the binding to
avoid collisions.

## 8. Template Strings

```lux
`count = ${count()}`
```

Lowering should:

- preserve static text exactly
- convert embedded expressions with `tostring(...)`
- support multiple interpolations
- support escaping of interpolation markers such as ``\${...}``

Example intent:

```lux
`${a}${b}`
```

lowers like:

```lua
tostring(a) .. tostring(b)
```

with constant folding allowed when parts are static.

## 9. Arrow Function `self` Shadowing

`->` always introduces a new leading parameter named `self`.

That new `self` shadows any outer variable with the same name inside the arrow
function body.

Example:

```lux
fn make(self) {
  return (x) -> self.foo + x
}
```

Inside the `->` function body, `self` refers to the implicit arrow parameter,
not the outer `make(self)` parameter.

This rule is intentionally simple and must remain predictable.

## 10. Lux Layering

### Language layer

- `fn`
- arrow functions
- implicit returns
- default parameters
- immutable `const` bindings
- destructuring local bindings
- expression statements
- do expressions
- conditional expressions
- optional chaining
- null coalescing
- compound assignment
- table spread
- pipeline

### Standard library layer

Do not patch global `table`, `string`, `math`, or GMod globals. Provide modules
instead:

```lux
import { arr, dict, str, num } from "lux/std"
import { valid, hookx, timerx } from "lux/gmod"

arr.map(xs, (x) => x * 2)
arr.filter(xs, (x) => x > 0)
arr.reduce(xs, 0, (acc, x) => acc + x)
arr.some(xs, (x) => x == target)
arr.every(xs, (x) => x.valid)
```

The stdlib is intentionally small and allocation-aware:

- `lux/std` exports pure modules such as `arr`, `dict`, `set`, `str`, `num`,
  `func`, and `pool`
- `lux/gmod` exports GMod-specific modules such as `valid`, `hookx`, `timerx`,
  `netx`, `json`, `players`, `entsx`, `color`, and `vgui`
- allocation-free variants use `Into` or `InPlace` naming, for example
  `arr.filterInto(xs, callback, out)` and `players.aliveInto(out)`
- no chain wrapper APIs are part of the core stdlib

See `docs/STDLIB.md` for the current API contract.

### Reactive layer

Provided by `lux/reactive`:

- `signal`
- `memo`
- `effect`

These are ordinary runtime imports. The core compiler does not understand or
special-case the reactive graph. Any future reactive optimization must be an
explicit package/host transform, not hidden core-language behavior.

### Host layer

Examples:

- `lux/ui`
- future non-UI hosts

## 11. UI Is Optional

Example UI code:

```lux
Column { gap = 12, padding = 16 } {
  Label { text = `Count: ${count()}` },
  Button {
    text = count() > 10 then "Big" else "Add",
    onClick = () => count(count() + 1)
  }
}
```

But Lux should also be useful without any UI host:

```lux
fn positives(xs) =
  arr.filter(xs, (x) => x > 0)
```

## 12. Lux-Specific Optimization Hooks

When a host plugin is enabled, the compiler may perform domain-specific
lowering, for example:

- static node hoisting
- specialized host codegen for known UI components
- `Show` / `For` block lowering
- static prop lifting
- host-specific layout path selection

These optimizations belong to the host layer, not the language core.

## 13. Explicit Non-Goals for MVP 0.1

- indentation-sensitive syntax
- classes
- decorators
- static type system
- runtime type checking
- monkey-patching global `table`
- deep static analysis of arbitrary business code
