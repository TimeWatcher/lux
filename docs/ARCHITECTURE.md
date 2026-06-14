# Architecture

## 1. Compiler First

Lux uses an offline compiler named `luxc`.

- no in-game compilation
- no runtime AST interpretation in GLua
- output is plain Lua 5.1 / GLua code

## 2. Layered Model

### Core language

The language layer is responsible for:

- parsing Lux syntax
- lowering Lux-only syntax to a normalized intermediate form
- preserving Lua-like semantics where practical

### Standard library

`lux/std` provides pure Lua/GLua-safe helper modules such as:

- `arr`
- `dict`
- `set`
- `str`
- `num`
- `func`
- `pool`

This layer should stay lightweight, allocation-aware, and runtime-cheap. It
must not patch Lua globals.

`lux/gmod` provides GMod-specific helper modules such as:

- `valid`
- `hookx`
- `timerx`
- `netx`
- `players`
- `entsx`
- `vgui`

This layer may depend on GMod globals, but it should remain thin and
lifecycle-aware rather than hiding core GMod APIs behind a large framework.

### Reactive runtime

`lux/reactive` provides:

- `signal`
- `memo`
- `effect`

This layer is general-purpose and should not depend on UI. It is a normal
runtime package: the compiler must not special-case signals, memos, effects, or
reactive graph scheduling in the core language.

### Host plugins

Host plugins add domain-specific lowering and runtime integration.

Examples:

- `lux/ui`
- future data/config/network hosts

### Macro providers

Macro providers add syntax-level rewrites before resolution and lowering.

They are compiler extensions, not runtime modules. A macro provider receives
structured AST and returns structured AST; it should not inject raw Lua strings.

### Dependency direction

The intended dependency flow is one-way:

```text
core compiler
  -> std runtime
  -> reactive runtime
  -> host runtime
```

More concretely:

- `lux/ui` may depend on `lux/reactive`
- `lux/reactive` must never depend on `lux/ui`
- the core language/compiler must not know host implementation details

## 3. UI as a Host, Not the Core

The UI layer must remain optional.

When a compile-time host package declares an explicit host contract, for example
`target = "lux/ui"` and `runtime = "lux/ui"`, the compiler may let that package
recognize known constructs and lower them aggressively:

- static node folding
- `children` folding from callable chaining
- `Show` / `For` block specialization
- direct VGUI host code generation

Without `lux/ui`, Lux code should still be valuable for ordinary GLua logic and
tooling.

Host transforms must run after import/name resolution and match symbol origin,
not just raw identifier spelling. A binding imported from `lux/ui` may be
transformed differently from an unrelated user-local binding with the same
name.

Transform support code should live in runtime packages. A transform can consume
large source-level imports from its `target` module and inject narrower runtime
imports through `ctx.importRuntime(...)`. The GMod backend decides runtime
artifacts from the transformed IR.

## 4. Practical Priorities

### Language priorities

- better syntax without losing Lua familiarity
- explicit semantics
- low-overhead lowering

### Runtime priorities

- small generated code
- minimal helper overhead
- no unnecessary allocation churn

### Tooling priorities

- fast incremental builds
- reliable diagnostics
- debuggable output
- useful source correlation between `.lux` and generated Lua

## 5. Diagnostics and Source Correlation

This is not optional for real-world GLua use.

`luxc` must emit useful source correlation metadata so runtime and compile-time
errors can be mapped back to Lux source locations.

Possible forms include:

- generated source line comments
- sidecar source map metadata
- stable line-preserving code generation where practical

At minimum, generated Lua should preserve useful line correlation where
possible.

## 6. Implementation Shape

The current intended project split is:

- `compiler/` - parser, AST, lowering, codegen, CLI
- `packages/` - std library, reactive layer, macros, and host/runtime phases
- `examples/` - source and generated output samples
- `docs/` - language and system specs
