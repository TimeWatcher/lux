# AST and Normalization Draft

This document defines the intended parser-facing AST shape for Lux MVP 0.1, and
the small amount of normalization expected before code generation.

## 1. Design Goals

- preserve source intent where it matters
- keep new Lux syntax explicit in the tree
- support host-aware lowering without coupling the core AST to UI
- make nil-safe chain lowering practical
- make implicit-return blocks explicit in the tree
- keep brace meaning context-driven instead of content-guessed

## 2. Compilation Stages

The intended stages are:

1. source text
2. lexer tokens
3. surface AST
4. macro expansion
5. scope and import resolution
6. normalized IR
7. host transforms
8. backend code generation

For MVP 0.1, the difference between the surface AST and the normalized IR can
stay small, but it is still useful to distinguish them conceptually.

## 3. Node Metadata

Every surface AST node should carry source location information.

Minimum shape:

```text
SourceSpan
  file_id: FileId
  byte_start: u32
  byte_end: u32
  line_start: u32
  line_end: u32
```

Normalized IR nodes should retain an origin reference:

```text
Origin
  = SourceOrigin(span: SourceSpan)
  | SyntheticOrigin(span: SourceSpan, reason: String)
```

This is required for parser diagnostics, lowering diagnostics, host-transform
diagnostics, and runtime source correlation.

## 4. Top-Level Nodes

### Module

```text
Module
  body: [Stmt]
```

Lux can remain file-oriented in MVP 0.1.

### Block

```text
Block
  statements: [Stmt]
  tail: Expr?
```

`tail` is the key to implicit expression return.

Examples:

```lux
fn sum(a, b) { a + b }
if ok { value } else { fallback }
```

Both forms produce a block whose final expression sits in `tail`.

### Block value rule

A block used in expression position has a value.

Its value is the final expression statement.

If the final item is not an expression statement, the block value is `nil`.

If a final expression is followed by a semicolon before the closing brace, it
is parsed as an `ExprStmt` in `statements`, not as `tail`.

The same structural representation is reused for function bodies, where the
tail may become an implicit return.

That does not mean every function body block gains a synthetic `return nil`.
Only an actual final expression tail is eligible for implicit return lowering.

### Function-body implicit return rule

A function body may only expose an implicit return tail when its final item is
an expression statement and control reaches it naturally.

Examples:

```lux
fn a() {
  1 + 2
}
```

normalizes as though the block has:

```text
statements = []
tail = BinaryExpr(...)
```

But:

```lux
fn b() {
  local x = 1
  x += 2
}
```

has no implicit return tail, because compound assignment is a statement, not an
expression.

```lux
fn c() {
  1 + 2;
}
```

also has no implicit return tail, because the trailing semicolon suppresses
tail expression recognition.

## 5. Statements

### Core statements

```text
LocalDeclStmt
  mode: Local | Const
  names: [Binding]
  values: [Expr]

LocalDestructureStmt
  mode: Local | Const
  patterns: [Pattern]
  values: [Expr]

AssignStmt
  targets: [Assignable]
  values: [Expr]

CompoundAssignStmt
  target: Assignable
  op: CompoundAssignOp
  value: Expr

ExprStmt
  expr: Expr

ReturnStmt
  values: [Expr]

BreakStmt

ImportStmt
  source: String
  specifiers: [ImportSpecifier]
  side_effect_only: bool
  phase: ImportPhase

ExportDeclStmt
  decl: Stmt

ExportListStmt
  names: [Identifier]

HostPackageDeclStmt
  target: String
  runtime: String
```

Where:

```text
ImportPhase = Runtime | Macro

ImportSpecifier
  = Named(imported: Identifier, local: Identifier)
  | Namespace(local: Identifier)
```

Examples:

```lux
import { arr } from "lux/std"
import { arr as array } from "lux/std"
import * as std from "lux/std"
import "setup"
import macro { dbg } from "lux/macros"
import macro * as gmodMacros from "lux/gmod/macros"
```

Macro imports are compile-time only. They do not create runtime module edges,
and macro bindings are invalid as runtime values.

### Function declaration

```text
FunctionDeclStmt
  name: FunctionName
  params: [Identifier]
  vararg: bool
  body: Block
```

For MVP 0.1, `fn` declaration lowering may normalize directly into this form.

### Function names

GLua-friendly declaration names are part of MVP:

```text
FunctionName
  = SimpleName
  | DottedName
  | MethodName
```

Examples:

```lux
fn foo(a) { ... }
fn M.foo(a) { ... }
fn PANEL:Paint(w, h) { ... }
```

These correspond to:

- lexical function declaration lowering
- dotted table-field function assignment lowering
- Lua method declaration lowering with `:`

Semantic rules:

- `SimpleName` declares a lexical binding in the current lexical scope
- at module top level, that lexical binding is module-private by default
- inside nested scopes, that lexical binding is local to the containing scope
- `DottedName` is a side-effecting table-field declaration/assignment
- `MethodName` is a side-effecting method declaration on an existing receiver path

In MVP 0.1, `export fn ...` is only valid when the wrapped declaration uses
`SimpleName`.

Compile-time packages may use phase-qualified export declarations:

```text
ExportDecl
  kind: Runtime | Macro | HostExpr
  stmt: FunctionDecl | LocalDeclStmt(mode = Const)

HostPackageDecl
  target: String
  runtime: String
```

Surface forms:

```lux
export fn helper(...) { ... }
export macro fn defineHook(ctx, call) { ... }
export host package {
  target = "lux/ui",
  runtime = "lux/ui"
}
export host expr fn foldNode(ctx, call) { ... }
```

`export macro fn` declares a user-callable syntax macro for `import macro`.
`export host expr fn` declares an expression host transform. These
phase-qualified exports are compile-time package declarations, not runtime
module exports.

`export host package` declares the host package contract used by all host
transforms in that compile-time module. `target` is the runtime source module
whose imported symbols may be transformed. `runtime` is the runtime source
module used by transform-injected imports such as `ctx.importRuntime(...)`.

### Lua-compatible control statements

The parser should carry dedicated statement nodes for:

```text
IfStmt
DoStmt
WhileStmt
NumericForStmt
GenericForStmt
RepeatUntilStmt
```

These are part of the intended Lua-compatible statement surface for MVP 0.1.

## 6. Expressions

### Basic expressions

```text
IdentifierExpr
NilLiteralExpr
BooleanLiteralExpr
NumberLiteralExpr
StringLiteralExpr
TemplateStringExpr
TableExpr
ParenExpr
VarargExpr
```

### Unary and binary expressions

```text
UnaryExpr
  op: UnaryOp
  argument: Expr

BinaryExpr
  op: BinaryOp
  left: Expr
  right: Expr
```

`BinaryOp` covers:

- `+`, `-`, `*`, `/`, `%`, `^`
- `..`
- `==`, `~=`, `<`, `<=`, `>`, `>=`
- `and`, `or`
- `??`

It is acceptable for the normalized IR to split `??` into a dedicated
`CoalesceExpr`, but the parser does not strictly need that distinction.

### Conditional expressions

```text
ConditionalExpr
  condition: Expr
  then_branch: ExprOrBlock
  else_branch: ExprOrBlock
  form: ConditionalForm
```

Where:

```text
ConditionalForm = IfExpr | ThenElse
ExprOrBlock = Expr | Block
```

This lets the parser preserve source style while the normalized IR can later
erase the surface distinction.

### Function expressions

```text
FunctionExpr
  params: [Identifier]
  vararg: bool
  body: Block
  arrow_kind: ArrowKind
```

Where:

```text
ArrowKind = Normal | ImplicitSelf
```

This is the AST-level representation for:

```lux
(a) => a + 1
(w, h) -> draw(self, w, h)
```

During normalization, `ImplicitSelf` becomes an explicit leading `self`
parameter in the generated backend form.

That generated `self` shadows any outer variable named `self` inside the
function body.

### Lua multivalue compatibility

Lux must preserve Lua's multivalue behavior in Lua-sensitive positions.

Calls and `VarargExpr` may therefore remain multivalued in:

- the final slot of a `return` value list
- the final slot of a local declaration or assignment value list
- the final array-style field position of a table literal
- the final argument position of a call

Outside those positions, multivalue expressions are normalized down to a
single value exactly as Lua does.

Lux intentionally preserves Lua table-constructor multivalue expansion for the
final array field. For example, `{ 1, f() }` may expand all values returned by
`f()`, while `{ f(), 1 }` collapses `f()` to a single value.

This matters for examples such as:

```lux
fn passthrough() = f()
fn logAll(...) {
  return ...
}
```

### Table expressions

`TableExpr` needs explicit field shapes because both ordinary Lua data and
future host plugins depend on that distinction.

```text
TableExpr
  fields: [TableField]

TableField
  = ArrayField(expr: Expr)
  | NamedField(name: String, value: Expr)
  | ExprKeyField(key: Expr, value: Expr)
```

Examples:

```lux
{
  text = "Hi",
  [dynamicKey] = value,
  Label { text = "child" }
}
```

The last example is still just an array-style field containing a call
expression. Any later host folding must build on this structure rather than on
raw token guessing.

## 7. Chain Expressions

Plain nested `CallExpr(MemberExpr(...))` trees are not ideal for Lux because
optional chaining needs short-circuit-aware lowering without double evaluation.

So Lux should prefer a chain representation:

```text
ChainExpr
  base: Expr
  segments: [ChainSegment]
```

### Segment kinds

```text
MemberSegment
  name: String
  optional: bool

IndexSegment
  index: Expr
  optional: bool

CallSegment
  args: [Expr]
  style: CallStyle

SafeDotCallSegment
  name: String
  args: [Expr]
  style: CallStyle

MethodCallSegment
  name: String
  args: [Expr]
  optional: bool
  style: CallStyle
```

Where:

```text
CallStyle = Paren | TailTable | TailString
```

Examples:

```lux
mgfx?.RoundedBox(...)
player?:GetExp()
factory?.Make()()
tbl?.[key]
Label { text = "x" }
Foo { a = 1 } { b = 2 }
```

Possible chain sketches:

```text
ChainExpr(
  base = Identifier("mgfx"),
  segments = [
    SafeDotCallSegment(name = "RoundedBox", args = [...], style = Paren),
  ]
)
```

```text
ChainExpr(
  base = Identifier("player"),
  segments = [
    MethodCallSegment(name = "GetExp", args = [], optional = true, style = Paren),
  ]
)
```

```text
ChainExpr(
  base = Identifier("factory"),
  segments = [
    SafeDotCallSegment(name = "Make", args = [], style = Paren),
    CallSegment(args = [], style = Paren),
  ]
)
```

```text
ChainExpr(
  base = Identifier("tbl"),
  segments = [
    IndexSegment(index = Identifier("key"), optional = true),
  ]
)
```

```text
ChainExpr(
  base = Identifier("Foo"),
  segments = [
    CallSegment(args = [TableExpr(...)], style = TailTable),
    CallSegment(args = [TableExpr(...)], style = TailTable),
  ]
)
```

This last form is important because it keeps callable chaining generic. Host
plugins can pattern-match these chains later and fold them if appropriate.

The distinction between `SafeDotCallSegment` and a normal trailing `CallSegment`
is how MVP 0.1 keeps optional safety precise instead of making whole chains
implicitly safe.

### Parser boundary rules for safe chain segments

These cases must stay distinct:

```lux
obj?.name
obj?.name(args)
obj?:name(args)
(obj?.name)(args)
tbl?.[key]
```

They map conceptually to:

- `obj?.name` -> `MemberSegment(optional = true)`
- `obj?.name(args)` -> `SafeDotCallSegment`
- `obj?:name(args)` -> `MethodCallSegment(optional = true)`
- `(obj?.name)(args)` -> optional member access first, then a normal outer call
- `tbl?.[key]` -> `IndexSegment(optional = true)`

MVP 0.1 does not need a dedicated `SafeIndexCallSegment`. If users write:

```lux
tbl?.[key](args)
```

that means a safe indexed access followed by a normal call on the result.

## 8. Assignable Targets

Not every expression may appear on the left-hand side of assignment.

Use a restricted target category:

```text
Assignable
  = IdentifierTarget
  | MemberTarget
  | IndexTarget
```

Optional segments are never valid assignment targets.

Examples:

```lux
x += 1           -- valid
player.score = 5 -- valid
player?:GetExp() = 5 -- invalid
```

### Normalized place abstraction

Compound assignment and other read-modify-write transforms should not operate
directly on raw syntax. Normalization should first translate an `Assignable`
into a single-evaluation place representation.

One acceptable conceptual shape is:

```text
PlaceRef
  setup: [Stmt]
  read: Expr
  write(value: Expr): Stmt
```

Examples:

```lux
getPlayer().score += 1
tbl[getKey()] += 1
```

need a `PlaceRef` so receiver/key setup happens once before the rewritten read
and write steps are emitted.

## 9. Template Strings

Template strings should stay explicit in the tree:

```text
TemplateStringExpr
  parts: [TemplatePart]

TemplatePart
  = TextPart
  | ExprPart
```

Example:

```lux
`Count: ${count()}`
```

This gives later passes a chance to:

- lower to concatenation
- fold purely static templates
- recognize host-specific dynamic text bindings

## 10. Surface AST vs Normalized IR

The parser-facing AST may preserve more syntax detail than the backend needs.

A small normalization pass should do things like:

- choose explicit value/statement/return emission strategies for blocks
- convert `->` into an explicit `self` parameter form
- hoist simple-name `fn` bindings within a lexical scope
- normalize `then/else` and `if` expressions into one conditional IR
- validate assignment targets
- annotate chain short-circuit boundaries
- mark implicit-tail-return blocks
- specialize statement-position conditional expressions into direct statement IR
- preserve Lua multivalue where context requires it
- lower side-effect imports into binding-free module edges
- resolve imported symbol origin for host transforms

### Block evaluation contexts

The surface AST can keep a single `Block` node, but normalized IR should make
its evaluation strategy explicit.

One acceptable conceptual shape is:

```text
BlockValue
  statements: [Stmt]
  tail: Expr?
  fallback: Nil
```

with explicit emission modes such as:

- `emitBlockAsStatements(block)`
- `emitBlockAsReturn(block)`
- `emitBlockInto(block, targetTemp)`

This avoids scattering ad hoc temporary-variable logic across every lowering
site that needs block values.

### Conditional lowering by context

`ConditionalExpr` keeps one surface syntax shape, but normalization/codegen must
branch by context:

- in statement position, lower directly to `IfStmt`
- in value position, lower into an explicit destination temp/place
- in return position, lower to branch-local `return` when possible

That distinction is required for correct code generation in Lua, which has
statement `if` but no native `if` expression.

### Function declaration lowering modes

Normalized lowering should distinguish between:

- lexical simple-name declarations
- dotted table-field declarations
- method declarations

Simple-name declarations participate in lexical binding hoisting for their
scope. Dotted and method declarations do not; they preserve normal statement
execution order because they mutate existing runtime objects/tables.

The semantic model for a hoisted lexical function declaration is:

```text
1. predeclare binding in scope
2. assign function value in source order
```

This makes forward references and mutual recursion work without implying that
arbitrary local initializers are hoisted.

### Resolved symbols for host transforms

Host-aware lowering must not pattern-match raw identifier text alone.

After scope/import resolution, identifier-like references should carry binding
identity:

```text
ResolvedSymbol
  local_name: String
  binding_kind: Local | Param | Global | Import
  source_module: String?
  imported_name: String?
```

This lets a host plugin distinguish:

- `Column` imported from `lux/ui`
- a user-local variable also named `Column`

without accidental rewrites.

### Gensym hygiene

Normalization and code generation will introduce temporaries such as
`__lux_obj_1` and `__lux_tmp_2`.

Those names must be generated through a scoped gensym facility rather than by
hardcoded strings. The gensym allocator should:

- use a reserved Lux prefix
- keep a module-local counter
- avoid collisions with already-bound user identifiers in scope

The normalized IR should still remain close to Lua codegen needs.

## 11. Host-Aware Lowering Boundary

The AST must remain host-agnostic.

That means:

- there is no `UiNodeExpr` in the core AST
- there is no `ChildrenBlockExpr` in the core AST
- `Foo { ... } { ... }` remains just a call chain in the core tree

Host plugins, such as `lux/ui`, may later interpret certain imports or symbols
and lower recognized patterns into domain-specific internal forms.

The core compiler must not guess that a second tail block means `children`
unless an explicit host transform says so for a known symbol.

Host transforms may add explicit runtime requirements to IR, for example a
small runtime import needed by a UI transform. Codegen should emit imports from
those requirements, not by scanning for magic identifier names.

The host transform context returns the actual injected local name:

```text
ctx.importRuntime(imported: String, preferredLocal: String) -> String
```

The returned name may differ from `preferredLocal` if the preferred binding is
already occupied in the module. Transforms should use the returned value when
building identifier IR.

This keeps Lux useful outside UI code.
