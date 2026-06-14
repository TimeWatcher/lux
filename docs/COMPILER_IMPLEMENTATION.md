# Compiler Implementation Blueprint

This document turns the Lux MVP 0.1 language spec into a concrete compiler
implementation plan.

The goal is not just "have passes", but to define:

- what each pass owns
- what data structure it consumes and produces
- where grammar ambiguity is resolved
- where Lua 5.1 / GLua compatibility is preserved
- where host plugins are allowed to intervene

## 1. End-to-End Pipeline

Recommended production pipeline:

1. source loading
2. tokenization
3. surface parsing
4. diagnostics + error recovery
5. macro expansion
6. scope and import resolution
7. project/module graph linking
8. normalized IR lowering with origins
9. optional host transforms on IR
10. backend planning
11. Lua/GLua code generation through a source-map-aware writer
12. source-correlation emission
13. optional target packaging such as `.gma`

In types:

```text
SourceFile
  -> TokenStream
  -> SurfaceAst
  -> MacroExpandedAst
  -> ResolvedAst
  -> ModuleGraph
  -> CoreIr
  -> HostAdjustedIr
  -> BackendPlan
  -> LuaChunk + SourceMap
  -> output files / packages
```

The important principle is that each stage should remove one category of
ambiguity:

- tokenizer removes character-level ambiguity
- parser removes grammar ambiguity
- resolver removes binding-origin ambiguity
- module graph linking removes import-source ambiguity
- lowering removes semantic/context ambiguity
- codegen only emits Lua; it should not still be deciding language meaning
- packaging only consumes backend artifacts; it should not re-run language logic

Current implementation status: `compiler/src/codegen/lua.rs` now emits from
`IrModule` through `LuaWriter`, and the CLI compile path runs
`parse -> macro expansion -> resolve -> lower -> host transform -> codegen`.
The old AST-direct emission direction is no longer the main path. Further
semantic rewrites should be added in macro expansion, lowering, or backend/host
planning, not by reintroducing surface-AST codegen decisions.

Project compilation now adds an offline module graph stage before lowering
project artifacts. The graph canonicalizes Lux import strings into stable module
ids such as `shared/foo`, rejects missing/cyclic modules, and enforces GMod
realm boundaries before generated Lua is written.

The graph also validates named imports against the resolved target module's
export table. For example, `import { foo } from "bar"` fails during project
compilation if `bar` does not export `foo`.

Runtime external artifacts are computed after host transforms from the final IR.
Runtime libraries such as `lux/std` and `lux/ui` live as runtime phases under
`packages/`; Rust discovers phase sources by directory convention and loads
export metadata for graph validation. The GMod backend compiles only the
referenced runtime packages into generated artifacts. Host transforms may
rewrite calls to runtime package APIs, but runtime behavior itself should not
be embedded in the Rust code generator.

## 2. Suggested Rust Layout

For MVP, prefer one Rust crate with modules over many micro-crates. Splitting
too early usually slows down parser/compiler work.

Suggested layout:

```text
compiler/
  Cargo.toml
  src/
    main.rs
    lib.rs
    cli/
      mod.rs
    source/
      mod.rs
      file.rs
      span.rs
    diag/
      mod.rs
      diagnostic.rs
      emitter.rs
    lex/
      mod.rs
      token.rs
      lexer.rs
      template.rs
    parse/
      mod.rs
      parser.rs
      stmt.rs
      expr.rs
      recover.rs
    ast/
      mod.rs
      nodes.rs
      visit.rs
    resolve/
      mod.rs
      scope.rs
      symbols.rs
      imports.rs
    ir/
      mod.rs
      core.rs
      place.rs
      value_ctx.rs
    lower/
      mod.rs
      blocks.rs
      chains.rs
      functions.rs
      multivalue.rs
      statements.rs
    packages.rs
    host/
      mod.rs
      registry.rs
      transform.rs
    codegen/
      mod.rs
      lua_writer.rs
      emit_expr.rs
      emit_stmt.rs
      source_map.rs
    gmod/
      mod.rs
      backend.rs
      loader.rs
      package.rs
```

If the project grows later, the first clean split is usually:

- `luxc` binary
- `luxc_core` library

not dozens of crates.

## 3. Source and Diagnostics Foundations

Before writing the parser, define these core primitives:

```text
FileId
SourceFile
SourceSpan
Diagnostic
Severity
Label
```

Recommended `SourceSpan`:

```text
SourceSpan
  file_id: FileId
  byte_start: u32
  byte_end: u32
  line_start: u32
  line_end: u32
  column_start: u32
  column_end: u32
```

Every token and every AST node should carry a span.

Diagnostics should support:

- primary label
- secondary labels
- notes
- help text

This matters early because lexer and parser recovery are much easier to design
when diagnostics are first-class rather than bolted on later.

## 4. Tokenizer

## 4.1 Responsibilities

The tokenizer owns:

- longest-match operator recognition
- keyword vs identifier classification
- template-string mode switching
- trivia skipping or collection
- source spans

The tokenizer does **not** own:

- block vs table brace meaning
- expression vs statement meaning
- safe-dot-call vs optional member semantics

Those belong to the parser.

## 4.2 Token Structure

Recommended token shape:

```text
Token
  kind: TokenKind
  span: SourceSpan
```

Optionally:

```text
Token
  kind: TokenKind
  span: SourceSpan
  leading_trivia: TriviaIdRange
```

For MVP, trivia can stay out-of-band if formatting is not implemented yet.

## 4.3 Lexer Modes

Lux needs at least three lexer modes:

```text
Normal
TemplateString
TemplateExpr
```

Behavior:

- `Normal` lexes ordinary code
- encountering `` ` `` enters `TemplateString`
- encountering `${` inside template text enters `TemplateExpr`
- closing `}` returns from `TemplateExpr` to `TemplateString`
- closing `` ` `` exits template mode back to `Normal`

Use an explicit mode stack rather than ad hoc flags.

## 4.4 Core Lexing Strategy

Use a byte-oriented cursor with helper methods:

```text
peek()
peek_n(n)
bump()
match_char(c)
match_str("...")
start_span()
finish_span()
```

Operator families should be lexed with specialized helpers:

- dot family: `.`, `..`, `...`, `..=`
- question family: `?.`, `?:`, `??`
- colon family: `:`
- minus family: `->`, `-=`, `-`
- equals family: `=>`, `==`, `=`

This keeps the main loop clean and makes it harder to break longest-match
behavior later.

## 4.5 Important Lexer Errors

The lexer should directly report:

- bare `?`
- unterminated string
- unterminated template string
- unterminated block comment
- invalid number literal

Recovery rule:

- emit a diagnostic
- consume until the token can be safely resumed

## 5. Parser

## 5.1 Recommended Style

Use a hand-written recursive-descent parser with Pratt-style expression parsing.

That gives Lux the right mix of:

- context-sensitive statement parsing
- precise brace disambiguation
- custom postfix chain parsing
- explicit diagnostics and recovery

Avoid parser generators for MVP. Lux has too many context-shaped edges for that
to be pleasant this early.

## 5.2 Parser State

Recommended parser state:

```text
Parser
  tokens: &[Token]
  index: usize
  diagnostics: Vec<Diagnostic>
```

Helpers:

```text
current()
peek(n)
at(kind)
eat(kind)
expect(kind)
bump()
recover_until(...)
```

## 5.3 Statement Parsing

Top-level entry:

```text
parse_module() -> Module
```

Statement dispatcher:

```text
parse_stmt() -> Stmt
```

Important statement parsers:

```text
parse_local_decl_stmt()
parse_return_stmt()
parse_break_stmt()
parse_import_stmt()
parse_export_stmt()
parse_if_stmt()
parse_while_stmt()
parse_for_stmt()
parse_repeat_stmt()
parse_do_stmt()
parse_fn_decl_stmt()
parse_assignment_or_expr_stmt()
```

The last one is where many dynamic-language parsers get messy. Lux should parse
an expression first, then decide whether the result is:

- an assignable left-hand side followed by assignment
- an assignable left-hand side followed by compound assignment
- otherwise an expression statement

## 5.4 Expression Parsing

Use precedence-based Pratt parsing:

```text
parse_expr(min_bp: BindingPower) -> Expr
```

Expression parsing should be split conceptually into:

1. prefix / primary parse
2. postfix chain parse
3. infix loop

That separation matters because Lux's hardest syntax lives in postfix chains and
brace calls, not in arithmetic.

## 5.5 Brace Disambiguation

This is one of the main parser responsibilities.

Rules:

- when parsing a control structure body or function body, `{}` means block
- when parsing a postfix call continuation after an expression, `{}` means tail table call
- when parsing a primary expression, `{}` means table literal

Do not inspect contents like `=` to guess.

Practically, that means:

- `parse_block()` is only called from grammar positions that demand a block
- `parse_table_expr()` is only called from expression-primary position
- postfix parser may consume `{ ... }` as tail-call argument only after an expression

## 5.6 Function Declarations

Function declarations need three syntactic forms:

```text
fn foo(...)
fn A.B(...)
fn A:B(...)
```

Recommended parse result:

```text
FunctionDeclStmt
  name: FunctionName
  params: [Identifier]
  vararg: bool
  body: Block
  span: SourceSpan
```

`FunctionName` should preserve:

- simple name
- dotted path
- method path

Do not lower these differences away in the parser.

## 5.7 Arrow Functions

Arrow functions should parse as expressions only.

Recommended approach:

- parse parenthesized parameter list
- if followed by `=>` or `->`, reinterpret as function expression
- otherwise keep as normal parenthesized expression

This is one of the few places where limited backtracking or lookahead is worth
it.

## 5.8 Conditional Syntax

Lux has both:

```lux
if cond { a } else { b }
cond then a else b
```

Both should parse into the same `ConditionalExpr` node shape with a `form`
field preserving the source surface.

Do not lower them apart in the parser.

## 5.9 Chain Parsing

Postfix chain parsing is where the optional operators must be pinned down.

Suggested loop after primary parse:

```text
while next token can extend a postfix chain:
  parse one segment
```

Segment cases:

- `.name`
- `?.name`
- `:[not supported in core expression syntax]`
- `?:name(args)`
- `[expr]`
- `?.[expr]`
- `(args...)`
- `{table}`
- `string-literal-tail`

Critical distinction:

- `obj?.name` -> optional member segment
- `obj?.name(args)` -> safe dot call segment
- `(obj?.name)(args)` -> optional member, then normal outer call

This requires the postfix parser to inspect whether `?.name` is immediately
followed by call syntax.

## 5.10 Table Parsing

Recommended table field forms:

- `name = expr`
- `[expr] = expr`
- `expr`

The parser should emit:

```text
NamedField
ExprKeyField
ArrayField
```

This is important for both Lua compatibility and later host transforms.

## 5.11 Error Recovery

Recommended recovery boundaries:

- statement terminators / next statement starters
- closing `}`
- closing `)`
- `else`
- `until`
- EOF

MVP parser recovery does not need to be heroic. It just needs to avoid turning
one syntax mistake into fifty nonsense errors.

## 6. Surface AST

The surface AST should stay close to syntax.

Keep these distinctions:

- `ConditionalExpr(form = IfExpr | ThenElse)`
- `FunctionName(Simple | Dotted | Method)`
- `ArrowKind(Normal | ImplicitSelf)`
- `CallStyle(Paren | TailTable | TailString)`
- `ImportStmt(side_effect_only)`
- `MemberSegment(optional)`
- `IndexSegment(optional)`
- `SafeDotCallSegment`
- `MethodCallSegment(optional)`

Do **not** force the parser AST to look like Lua yet.

## 7. Resolver

## 7.1 Responsibilities

The resolver owns:

- lexical scope creation
- binding creation
- symbol lookup
- import binding registration
- export validation
- simple-name `fn` hoisting
- symbol-origin metadata for host transforms

It does **not** own Lua code generation.

## 7.2 Scope Model

Recommended scope kinds:

```text
ModuleScope
FunctionScope
BlockScope
LoopScope
```

Bindings:

```text
BindingKind
  = Local
  | Param
  | Function
  | Import
  | Export
```

## 7.3 Hoisting Rule

Only simple-name `fn` declarations are hoisted as bindings.

Meaning:

1. collect lexical `fn foo(...)` names in a scope before resolving bodies
2. insert bindings into the scope
3. resolve bodies with those bindings visible

Do not hoist:

- `local x = ...`
- `fn A.B(...)`
- `fn A:B(...)`

Those preserve ordinary statement execution order.

## 7.4 Import Resolution

`import { x } from "m"` creates local import bindings.

`import { x as y } from "m"` creates local binding `y` with imported name `x`.

`import * as ns from "m"` creates a namespace binding to the module export
table.

`import "m"` creates no local bindings, but it still creates a module edge in
the graph.

`import macro { x } from "m"` and `import macro * as ns from "m"` create
compile-time macro bindings. They do not create runtime module edges.

Resolver output should attach import provenance:

```text
ResolvedSymbol
  local_name: String
  binding_kind: Local | Param | Global | Import
  source_module: String?
  imported_name: String?
```

This is what host transforms must match on.

Macro bindings use a distinct binding kind and are rejected if referenced as
runtime values after expansion.

## 8. Normalized IR

The normalized IR should be closer to code generation needs than the AST, but
it still should not be raw Lua source text.

Recommended responsibilities:

- remove surface-syntax duplication
- make evaluation context explicit
- preserve Lua multivalue where needed
- make single-evaluation rewrites explicit
- separate host transforms from syntax parsing

## 8.1 Block Strategies

Surface AST may use:

```text
Block
  statements: [Stmt]
  tail: Expr?
```

But normalized IR should add usage context.

One acceptable shape:

```text
BlockValue
  statements: [IrStmt]
  tail: IrExpr?
  fallback: Nil
```

Emission contexts:

- `Statement`
- `Value(target_place)`
- `Return`

Suggested helpers:

```text
emit_block_as_statements(block)
emit_block_into(block, target_place)
emit_block_as_return(block)
```

This is how Lux should unify:

- function implicit returns
- `if` expressions
- `then/else` expressions
- block values
- `??` lowering helpers

## 8.2 Conditional Lowering

Normalize conditional expressions into one IR form, but always preserve usage
context:

- statement context -> branch statements only
- value context -> branch assignment into destination
- return context -> branch-local returns

Do not route everything through discard temps.

## 8.3 Place Lowering

To implement compound assignment and other rewrites safely, lower assignables
into a place abstraction:

```text
PlaceRef
  setup: [IrStmt]
  read: IrExpr
  write(value: IrExpr): IrStmt
```

Examples:

```lux
getPlayer().score += 1
tbl[getKey()] += 1
```

Both need a `PlaceRef` before rewriting to read-modify-write form.

## 8.4 Chain Lowering

Optional chains should lower through staged temporaries, not inline stringly
templates.

Conceptually:

```text
ChainPlan
  setup: [IrStmt]
  result: IrExpr
  may_be_nil: bool
```

The plan needs to preserve:

- single evaluation of receiver
- single evaluation of index/key
- distinction between safe dot call and safe member + later normal call
- distinction between dot and colon calls

## 8.5 Multivalue Handling

Lux must preserve Lua multivalue only in Lua-sensitive positions:

- final return value
- final assignment value
- final local-decl value
- final array table field
- final call argument

Normalized IR should annotate whether an expression is emitted in:

```text
ValueMode = Single | MultiTail
```

Without this, later passes will accidentally collapse `f()` and `...`.

Current implementation preserves multivalue behavior in these Lua-sensitive
positions:

- final return value
- final assignment/local declaration value
- final generic-for iterator expression
- final function call argument
- final array field in a table constructor

Calls and `...` outside those positions are forced to single values by wrapping
them in parentheses, matching Lua 5.1 behavior.

The final table array field deliberately preserves Lua table-constructor
multivalue expansion. This is not accidental; it is part of Lux's Lua
compatibility contract.

Table spread is not Lua `pairs(src)` pasted directly into output. Since Lux
defines `nil` spread sources as ignored, codegen must lower each spread with an
explicit guard:

```lua
local src = base
if src ~= nil then
  for k, v in pairs(src) do
    out[k] = v
  end
end
```

The guard is required by language semantics, not an optimization detail.

## 8.6 Destructuring Lowering

Destructuring binds names first, evaluates each RHS once into a temporary, then
reads fields or array slots from that temporary.

It deliberately does not nil-protect the source:

```lux
local { name } = player
local [x] = point
```

lowers in the shape of:

```lua
local name
do
  local src = player
  local field = src.name
  name = field
end
```

If the source can be nil, user code should express that with `?? {}` or safe
access. This keeps destructuring aligned with ordinary Lua table access instead
of making a hidden optional-access operator.

## 8.7 Function Lowering Modes

Function declarations lower in three distinct ways:

### Lexical simple-name function

```lux
fn foo() = 1
```

Normalized intent:

```text
predeclare binding
assign function value in source order
```

### Dotted declaration

```lux
fn A.B() = 1
```

Normalized intent:

```text
evaluate receiver path as needed
assign function to field
```

### Method declaration

```lux
fn PANEL:Paint(w, h) { ... }
```

Normalized intent:

```text
assign method function with explicit self receiver semantics
```

These should not collapse into one generic form too early.

## 8.8 Do Expression Lowering

Value-position `do { ... }` should lower into scoped assignment when possible:

```lux
local x = do {
  local y = f()
  y + 1
}
```

Preferred shape:

```lua
local x
do
  local y = f()
  x = y + 1
end
```

Avoid generating an IIFE unless a later backend needs that shape for a specific
reason.

## 8.9 Export Lowering

`export fn foo(...)` means:

1. create the same lexical function binding as plain `fn foo(...)`
2. register that binding in module exports

`export const value = expr` means:

1. create the same immutable lexical binding as plain `const value = expr`
2. register that binding in module exports

`export { foo, bar }` means:

- verify `foo` and `bar` exist as lexical bindings
- register those bindings in export metadata / runtime output

## 9. Host Transform Boundary

Host transforms should run after:

- parsing
- macro expansion
- scope resolution
- import resolution
- core semantic normalization

Host transforms should run before:

- final Lua code generation

This is the sweet spot where:

- syntax meaning is already stable
- symbol origin is known
- transforms can still operate on structured IR

Never let host transforms parse raw syntax from scratch.

The implemented host registry is transform-list based. Host transforms are
registered by phase-qualified declarations in package host phases, for example
`export host expr fn foldNode(ctx, call)` in `packages/lux/ui/host/module.lux`;
future VGUI/reactive/data hosts should plug into the same registry rather than
adding host-specific syntax to the core language.

Host transforms return:

```text
HostTransformOutput
  module: IrModule
  diagnostics: [Diagnostic]
```

Transforms should express support-code needs as ordinary runtime package
imports. For example, the UI transform consumes imported component symbols such
as `Column`, then inserts an import of `node` from `lux/ui` and emits calls to
that helper. The actual helper is implemented in `packages/lux/ui/src/module.lux`.

This keeps the compiler boundary narrow: Rust owns structured IR rewriting,
symbol provenance checks, and backend packaging; runtime behavior belongs to Lux
packages that can be replaced or extended independently.

## 9.1 Macro Transform Boundary

Syntax macros run before name resolution and return structured AST.

The current registry is convention based:

```text
packages/<package-id>/compiletime/*.lux
packages/<package-id>/host/*.lux
```

Macro packages receive AST arguments and source spans through the compiler ABI,
and they report diagnostics through `MacroContext`. They must not return raw Lua
strings.

Default compile-time package phases currently include:

- `packages/lux/macros/compiletime/*.lux`
- `packages/lux/gmod/macros/compiletime/*.lux`
- `packages/lux/ui/host/*.lux`

Rust exposes only the stable compiler-side interface:

- `lux/compile/ast`
- `lux/compile/ir`

This keeps macros and host transforms self-contained Lux code. Adding new
macro/host packages should not require embedding package behavior in Rust.

## 9.2 Backend Planning Boundary

Backend planning runs after core lowering and host transforms.

For GMod, backend planning owns:

- realm assignment
- generated Lua file paths
- loader file plans
- `AddCSLuaFile` plans
- GMA packaging plans
- private module registry/import-linking plans

It must not own parser decisions, name resolution, or Lux semantic lowering.

## 10. Lua/GLua Codegen

## 10.1 Responsibilities

Codegen should:

- emit Lua 5.1 / GLua source
- respect source order and scoping
- preserve multivalue tail behavior
- use gensym-safe temporaries
- emit source correlation aids
- write through a source-map-aware `LuaWriter`

Codegen should not still be deciding:

- whether something is a block expression
- what `?.` means
- whether `Column` came from `lux/ui`

## 10.2 Module Shape

One acceptable module output shape:

```lua
local __lux_exports = {}

-- hoisted simple-name function bindings
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

Backend-specific glue can later decide how this chunk is loaded in shared,
client, or server environments.

## 10.3 Gensym

Use a central allocator:

```text
Gensym
  prefix: "__lux"
  next_id: u32
```

And make it aware of already-bound locals to avoid collisions.

Examples:

- `__lux_tmp_1`
- `__lux_obj_2`
- `__lux_key_3`

## 10.4 Source Correlation

At minimum, codegen supports:

- stable line-preserving emission where practical
- sidecar JSON source maps from `luxc compile --map`
- a writer API that can also support inline comments like
  `--#lux source: foo.lux:42`

Do not wait until "tooling phase" to preserve spans through codegen.

The production writer model is:

```text
LuaWriter
  output: String
  source_map: SourceMap
  line(text, origin)
```

Every IR node retains an `Origin`, and every meaningful generated line should
either reference a source origin or an explicit synthetic origin.

## 11. Concrete Pass Order

Recommended implementation order:

### Phase 1

- source files
- spans
- diagnostics
- tokens
- lexer tests

### Phase 2

- basic parser skeleton
- primary expressions
- postfix chains
- statements
- blocks
- parser tests

### Phase 3

- full surface AST
- function names
- table fields
- template strings
- import/export parsing

### Phase 4

- resolver
- scopes
- hoisted `fn` bindings
- import provenance
- export validation

### Phase 5

- normalized IR
- block context lowering
- conditional lowering
- place lowering
- optional chain lowering
- multivalue rules

### Phase 6

- Lua emitter
- export table emission
- source comments
- golden output tests

### Phase 6.5

- parser recovery at statement boundaries after diagnostics
- lint diagnostics for newline tail-table calls and semicolon-suppressed returns
- conservative lossless formatter with `--check` and `--write`; it preserves
  token text, comment text, and parsed AST shape while only rewriting
  whitespace/indentation; unsafe formatter output is rejected before write-back
- generated-line to Lux-source mapping through sidecar source maps
- centralized expression emission modes for statement/value/return lowering

### Phase 7

- host registry
- host transform API
- first `lux/ui` experiments

## 12. Testing Strategy

Every stage should have focused tests.

### Lexer tests

- longest-match families
- template strings
- `?.`, `?:`, `?.[`
- `=>`, `->`

### Parser tests

- brace disambiguation
- `fn foo`, `fn A.B`, `fn A:B`
- `obj?.name(args)` vs `(obj?.name)(args)`
- `tbl?.[key](args)`
- chained tail table calls
- `then/else` associativity

### Resolver tests

- forward function references
- mutual recursion
- shadowing
- export validation
- host import provenance

### Lowering tests

- `if` in statement/value/return context
- compound assignment single evaluation
- safe field/index/method lowering
- `??` with `false`
- multivalue preservation

### Codegen tests

- emitted Lua shape
- temp naming stability
- line correlation comments

## 13. Example Walkthrough

Input:

```lux
fn helper(x) = x + 1

export fn choose(player) =
  (player?:GetExp() ?? 0) > 5 then helper(10) else 0
```

Surface AST sketch:

```text
Module
  FunctionDeclStmt(SimpleName("helper"), ...)
  ExportDeclStmt(
    FunctionDeclStmt(SimpleName("choose"), ...)
  )
```

Resolved facts:

- `helper` is a module-private lexical function
- `choose` is a module-private lexical function that is also exported
- `player` is a parameter binding

Normalized sketch:

```text
predeclare helper
predeclare choose

helper = fn(x) -> return x + 1

choose = fn(player):
  if (safe_method_call(player, "GetExp") ?? 0) > 5 then
    return helper(10)
  else
    return 0
```

Lua-ish output:

```lua
local __lux_exports = {}
local helper
local choose

helper = function(x)
  return x + 1
end

choose = function(player)
  local __lux_obj = player
  local __lux_left = nil

  if __lux_obj ~= nil then
    local __lux_method = __lux_obj.GetExp
    if __lux_method ~= nil then
      __lux_left = __lux_method(__lux_obj)
    end
  end

  if (__lux_left ~= nil and __lux_left or 0) > 5 then
    return helper(10)
  else
    return 0
  end
end

__lux_exports.choose = choose
return __lux_exports
```

Note: the exact `?? 0` lowering must still use a real nil-test rather than Lua
`and/or`; the sketch above is only showing pass shape, not the final precise
coalesce implementation.

## 14. Recommended First Code Milestone

The first milestone should not be "full compiler".

It should be:

1. lex one file
2. parse one file into AST
3. resolve scopes/imports
4. lower simple lexical `fn`, `if`, `then/else`, `?.`, `??`, compound assign
5. emit Lua for non-host code

That gets Lux to a genuinely useful CLI milestone without dragging `lux/ui`
into the first implementation wave.
