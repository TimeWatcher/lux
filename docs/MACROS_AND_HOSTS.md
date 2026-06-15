# Macros and Host Transforms

Lux keeps language extension offline and self-contained:

- syntax macros run before resolution/lowering and rewrite surface AST
- host transforms run after resolution/lowering and rewrite normalized IR
- macro, host, and runtime implementations are Lux source files in installed
  package sets such as `lux-std`

Rust owns the compiler ABI, diagnostics, hygiene, source spans, sandboxed
evaluation, and structured AST/IR builders. Package behavior lives in Lux code.
There is no runtime compilation in GMod and no raw Lua string injection.

## 1. Compile-Time Packages

Compile-time packages are imported explicitly:

```lux
import macro { dbg } from "lux/macros"
import macro * as gmodMacros from "lux/gmod/macros"
```

Project builds may add package roots through `lux.toml`:

```toml
[gmod]
package_roots = "vendor/lux-std, vendor/project-packages"
```

Package roots are normally populated from `lux.lock` after `luxc install`, but
`package_roots` remains available for local vendoring and development
checkouts. Duplicate package ids are rejected instead of overwritten, so a
project cannot silently replace `lux/macros` or `lux/compile/macro`.

Each package is discovered by directory convention. The package id is its
directory path under the package root:

```text
vendor/lux-std/lux/ui/
  src/*.lux
  host/*.lux
```

Supported phases are:

- `src/`: runtime phase parts, compiled into generated Lua only when required by the module graph
- `compiletime/`: compile-time phase parts, evaluated offline for macro/helper packages
- `host/`: host phase parts, evaluated offline for host transforms

A package may combine runtime, compile-time, and host phases by adding the
corresponding directories. This is how `lux/ui` stays one logical package while
still separating shipped runtime code from offline host transform code.

Official `lux-std` compile-time package ids:

- `lux/macros`
- `lux/gmod/macros`
- `lux/compile/macro`
- `lux/compile/host`
- `lux/ui`

The compiler exposes intrinsic compile-time modules:

- `lux/compile/ast` for surface AST builders
- `lux/compile/ir` for normalized IR builders and match helpers

`lux/compile/macro` is a Lux-written helper facade over `lux/compile/ast`.
Macro authors should start there for common AST construction and diagnostics,
then drop to `lux/compile/ast` only when they need a lower-level node.

`lux/compile/host` is the equivalent Lux-written facade for host transforms. It
wraps the low-level host context and `lux/compile/ir` primitives with helpers
such as `host.importRuntimeIdent(...)`, `host.callRuntime(...)`, and
`host.tailTableParts(...)`.

Exported helper functions are not automatically user-facing macros. A
compile-time package must explicitly declare macro entrypoints with
phase-qualified exports:

```lux
export macro fn dbg(ctx, call) {
  ...
}
```

`export fn` exports compile-time helpers to other compile-time packages.
`export const` exports compile-time constants to other compile-time packages.
`export macro fn` exports a user-callable syntax macro. Helper packages such as
`lux/compile/macro` use ordinary runtime-phase exports, so macro authors can
import helpers and constants without exposing them through `import macro`.

`import macro` is checked against `export macro fn` declarations. A named macro
import fails immediately when the listed macro is not exported by that package,
even if the binding is never called. Namespace macro imports require the
package to expose at least one macro; individual namespace members are checked
when they are called. Unknown macro package ids are reported separately from
known helper packages that intentionally expose no user macros.

## 2. Expression Macros

Macros are first-class expression rewrites when they return `ast.expr(...)`.
The same macro can be used in local initializers, return values, table fields,
call arguments, nested expressions, or statement position:

```lux
local value = dbg(player?:GetExp() ?? 0)
return use(dbg(value))
local row = { dbg(a), label = dbg(name) }
```

Macro expansion returns structured AST:

```text
MacroExpansion
  = Expr(Expr)
  | Stmts([Stmt])
```

Position rules:

- in expression position, a macro must return `ast.expr(...)` or another
  expression expansion
- in statement position, a macro may return `ast.stmts(...)`, a single AST
  statement, or a statement array
- returning statement expansion in expression position is reported as
  `MACRO002`
- returning a non-AST value is a compile-time macro error at the call site

Macro authors should prefer the explicit wrappers `m.expr(...)` and
`m.stmts(...)` because they document intent and survive future helper changes.

If a macro needs setup statements plus a value, it should use
`m.exprWithSetup(statements, value, span)` or its intent-named alias
`m.lowerIntoTarget(statements, value, span)`:

```lux
local cached = m.localExpr(ctx, "value", call.args[1], call.span)

return m.exprWithSetup({
  cached.stmt,
  m.exprStmt(registerCall, call.span)
}, cached.ref, call.span)
```

The helper builds a Lux `do` expression. Lua codegen then lowers it according to
the surrounding target:

- return context becomes `do ... return value end`
- assignment/local-initializer context becomes `do ... target = value end`
- nested expression context uses a temporary, because Lua has no general
  statement-expression form

This avoids allocating a closure for hot-path expression macros. A macro may
still deliberately use `m.iifeBlock(block, span)` when an actual function
boundary is wanted.

The generated AST then goes through normal recursive macro expansion, resolver,
lowering, host transforms, and Lua codegen.

Expression macros should preserve source argument evaluation order. If a macro
needs to reuse arguments or return a value derived from one argument, cache
source arguments left-to-right inside the generated expression shape before
performing generated side effects. This keeps code such as
`m(makeEvent(), makeId(), makeCallback())` from silently becoming
`makeId(), makeEvent(), makeCallback()` after expansion.

## 3. Statement Macros

Macros may return `ast.stmts(...)` for statement expansion:

```lux
gmodMacros.defineNetReceiver("lux_msg", (len, ply) => handle(len, ply))
```

Statement expansions are valid only in statement position. If a statement macro
is used where a value is required, the compiler reports `MACRO002`.

## 4. Macro Context

Macro functions receive:

```text
fn macroName(ctx, call) -> MacroExpansion?
```

`ctx` exposes:

- `ctx.gensym(prefix)` for hygienic generated names
- `ctx.gensymString(prefix)` for runtime-visible unique strings
- `ctx.label(span)` for source-aware labels
- `ctx.error(code, message, span)` for diagnostics

Use `gensym` for identifiers that become locals or bindings in generated code.
Use `gensymString` for values users or host APIs may observe at runtime, such as
GMod hook ids. Keeping these separate prevents implementation names from
accidentally becoming public runtime protocol.

`gensymString` returns a sanitized runtime string, not a Lua identifier. Macro
authors should treat it as a user-observable protocol/id value and should not
feed it back into `ast.ident`.

`call` exposes:

- `source`
- `imported`
- `position`: `"expression"` or `"statement"`
- `argc`
- `args`
- `span`

Macro bindings are compile-time only. They do not create runtime module graph
edges and cannot be used as runtime values:

```lux
local x = dbg -- error
```

### Error Model

Compile-time code has three distinct failure channels:

- user diagnostics: call `ctx.error(code, message, span)` or helper
  `m.fail(...)`; these are reported at source locations and compilation
  continues where recovery is possible
- macro author errors: use a stable diagnostic code such as `MACRO_INTERNAL`
  when a helper can report misuse against a source span
- internal compiler errors: reserved for impossible compiler/ABI violations and
  implementation bugs; helper code such as `m.internalError(...)` should use
  this only for macro-library misuse that cannot be attributed to user source

Example: `m.expectPath(ctx, {}, span)` reports `MACRO_INTERNAL` through
`ctx.error`, while low-level `m.path({}, span)` uses `m.internalError(...)`
because passing an empty path is a macro helper bug.

## 5. Macro Helper Layer

`lux/compile/ast` is the stable low-level AST ABI. It is intentionally explicit,
but nested calls such as `ast.call(ast.member(ast.ident(...)))` are unpleasant
for day-to-day macro authoring.

`lux/compile/macro` provides a higher-level helper layer:

- `m.callPath({ "hook", "Add" }, args, span)`
- `m.callStmt({ "net", "Receive" }, args, span)`
- `m.localExpr(ctx, "tmp", expr, span)`
- `m.localMany(ctx, specs, span)`
- `m.constOne(name, value, span)`
- `m.fnDecl(name, params, body, span)`
- `m.exportRuntime("shared" | "client" | "server" | nil, stmt, span)`
- `m.doStmt(stmts, span)`
- `m.realmBlock("server" | "client" | "shared", stmts, span)`
- `m.serverBlock(stmts, span)`
- `m.clientBlock(stmts, span)`
- `m.sharedBlock(stmts, span)`
- `m.ifStmt(cond, thenStmts, elseStmts, span)`
- `m.guard("SOME_GLOBAL", stmts, span)`
- `m.block(stmts, tail, span)`
- `m.doExpr(stmts, tail, span)`
- `m.exprWithSetup(stmts, value, span)`
- `m.lowerIntoTarget(stmts, value, span)`
- `m.iifeBlock(block, span)`
- `m.data(expr)`
- `m.number(value, span)`
- `m.index(base, value, span)`
- `m.returnMany(values, span)`
- `m.expectArgCount(...)`
- `m.expectArgCount2(...)`
- `m.expectArgCounts(ctx, call, { 1, 2, 4 }, usage)`
- `m.requireStatement(...)`
- `m.requireStatementOrExpression(...)`
- `m.fail(ctx, code, message, span)`

`m.block(...)` constructs an AST block used by function bodies and IIFE bodies.
`m.doStmt(...)` constructs a Lua `do ... end` statement that scopes generated
locals. `m.doBlock(...)` remains available as an alias, but new macros should
prefer `doStmt` for clarity.
`m.doExpr(...)` constructs a Lux do-expression, and `m.exprWithSetup(...)`
wraps it as an expression macro expansion. `m.lowerIntoTarget(...)` is the same
shape with a name that emphasizes target-aware codegen.

`m.fnDecl(...)` and `m.exportRuntime(...)` let statement macros generate normal
runtime API declarations. This is intended for declaration-driven packages that
need Lux compile-time code generation without falling back to raw Lua strings.
`m.data(...)` reads a macro argument expression as declarative compile-time
data: strings, numbers, booleans, nil, tables, and identifiers. Identifiers are
converted to their names, so package DSLs can accept compact declarations such
as `{ name = Button, fields = { x, y, w, h } }`.

It also standardizes common macro diagnostics:

- `m.ERR_INVALID_ARITY` is `MACRO003`
- `m.ERR_INVALID_POSITION` is `MACRO004`

Macro packages may import other compile-time Lux packages. For example:

```lux
import * as m from "lux/compile/macro"
```

This keeps macro ergonomics in Lux source instead of embedding convenience APIs
in Rust. Realm-aware macros should build `RealmBlock` AST with
`m.serverBlock`, `m.clientBlock`, or `m.sharedBlock`; the GMod backend lowers
those blocks only into matching artifacts.

## 6. GMod Macro Defaults

GMod macros should be realm-safe by default by expressing realm intent in the
generated AST. For example, `defineNetReceiver(name, callback)` lowers to a
shared realm block that caches name and callback, nests a server realm block
for `util.AddNetworkString`, then registers `net.Receive` in the current
artifact.

GMod net macros are split by intent:

- `defineNetString(name)` declares a network string on the server
- `receiveNet(name, callback)` registers `net.Receive` without declaring a string
- `defineSharedNetReceiver(name, callback)` declares the string on the server and
  registers the receiver in the current realm
- `defineServerNetReceiver(name, callback)` places declaration and receiver
  registration in a server realm block
- `defineClientNetReceiver(name, callback)` places receiver registration in a
  client realm block
- `defineNetReceiver(name, callback)` is a convenience alias for
  `defineSharedNetReceiver`

Realm-specific macros place argument caching inside the realm block:

```lux
server {
  local name = makeName()
  local callback = makeCallback()
  util.AddNetworkString(name)
  net.Receive(name, callback)
}
```

That means `defineServerNetReceiver(makeName(), makeCallback())` does not call
`makeName()` or `makeCallback()` on the client. `defineSharedNetReceiver` is
intentionally different: it evaluates `name` and `callback` in shared context,
server-blocks `util.AddNetworkString`, and registers `net.Receive` in the
current artifact.

## 7. Host Transforms

Host transforms operate on normalized IR after symbol provenance is known. This
lets a transform distinguish imported UI components from user locals:

```lux
import { Column } from "lux/ui"
local view = Column { gap = 8 }
```

versus:

```lux
local Column = makeColumn
local view = Column { gap = 8 }
```

A host package has two explicit parts:

- a package contract, declared with `export host package`
- one or more phase-qualified host transform entrypoints

Example:

```lux
import * as ir from "lux/compile/ir"

export host package {
  target = "lux/ui",
  runtime = "lux/ui"
}

export host expr fn foldNode(ctx, call) {
  if call.imported != "Column" {
    return nil
  }

  local node = ctx.importRuntime("node", "__lux_ui_node")

  ir.call(ir.ident(node, call.expr), {
    ir.string(call.imported, call.expr),
    ...
  }, call.expr)
}
```

`target` is the source module whose imported runtime symbols this host can
transform. If user code imports `Column` from `"lux/ui"`, then a host package
whose `target` is `"lux/ui"` may inspect and rewrite calls through that binding.
If user code defines its own local `Column`, the transform does not run.

`runtime` is the runtime module used for transform-injected imports. It may be
the same as `target`, but it does not have to be. This lets a project expose a
friendly source module while keeping the generated runtime support in a
different module.

The transform receives:

```text
fn foldNode(ctx, call) -> IrExpr? | nil
```

`call` exposes:

- `source`
- `runtime`
- `imported`
- `local`
- `expr`

`ctx.importRuntime(imported, preferredLocal)` asks the host pipeline to keep or
inject a runtime import from the host package's `runtime` module. It returns
the actual local binding name that the transform must use in generated IR.
The compiler may return a different name than `preferredLocal` to avoid
colliding with user bindings or other generated imports.

For example, `lux/ui` folds component calls into calls to its runtime `node`
export while keeping `node` implemented in the `lux/ui/src/module.lux` package
source.

No legacy alias is provided. Host packages must use `ctx.importRuntime(...)`.

`call.imported` is the original exported name from the `target` module.
`call.local` is the local binding name at the user call site. For:

```lux
import { Column as Stack } from "lux/ui"
```

a transform sees `call.imported == "Column"` and `call.local == "Stack"`.
Host DSLs should generally use `call.imported` for semantic node/component
identity and `call.local` only when the source spelling matters.

`ir.tailTableParts(expr)` is the UI-style call-chain matcher used by `lux/ui`.
It returns `nil` unless the expression is one of these precise shapes:

```lux
Name { prop = value }
Name { prop = value } { Child { ... } }
```

The first table is treated as props and must not contain array-style fields.
The optional second tail table is treated as children and may contain
array-style child expressions. Ordinary calls such as `Name(1, 2)` and
ambiguous props such as `Name { dynamic }` are left untouched.

## 8. Package Boundary

Runtime, macro, and host code live in package sets, but phase sources remain
separate. Runtime phase files compile to ordinary Lua modules. Compile-time and
host phase files are evaluated offline and never ship as runtime Lua. Each
phase may be split into multiple `.lux` part files that share one package phase
module scope.

Dependency direction:

```text
core compiler ABI -> compile-time Lux package phases -> runtime Lux package phases
```

The core language does not know UI, VGUI, or reactive semantics. Those are host
layers built with the same macro/transform API.
