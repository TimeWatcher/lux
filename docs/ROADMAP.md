# Roadmap

## Phase 0 - Design

- define language MVP
- define operator precedence
- define AST node shapes
- define host plugin boundaries

## Phase 1 - Compiler Skeleton

- Rust workspace setup [implemented]
- lexer [implemented]
- parser [implemented]
- AST [implemented]
- macro expansion pipeline [implemented]
- scope / import resolution [implemented]
- basic diagnostics [implemented]
- Lua-compatible code generator [implemented]

## Phase 2 - Core Language Features

- `fn`
- brace blocks
- `=>` and `->`
- expression statements
- implicit expression return
- `if` expressions
- `then/else`
- `?.`, `?:`, `??`
- `?.[index]`
- compound assignment
- template strings
- varargs and Lua multivalue compatibility
- GLua-friendly dotted / method function declarations
- `import` / `export`
- namespace imports and macro imports

## Phase 3 - Runtime Libraries

- convention-based package phase registry from Lux source [implemented]
- runtime package dependency closure [implemented]
- `lux/std` [implemented]
- `lux/ui` [implemented]
- `lux/reactive` [implemented initial runtime]

## Phase 4 - Host Plugin Model

- host registration model [implemented through external compile-time Lux packages]
- host-aware lowering hooks [implemented]
- symbol-provenance matching [implemented]
- runtime package import injection for transform support code [implemented]
- first optional host: `lux/ui` [external compile-time package implemented]
- future user/WASM host providers

## Phase 5 - UI Host

- node factories in `packages/lux/ui/src/module.lux` [implemented outside luxc]
- reactive-backed UI runtime boundary [initial]
- specialized lowering for known UI constructs [initial tail-table chain folding]
- generated GLua/VGUI host code

## Phase 6 - Tooling

- watch mode
- incremental build
- error formatting [implemented baseline diagnostics]
- lint and conservative formatter [implemented]
- source correlation strategy [implemented source comments and sidecar maps]
- GMod manifest/build/package commands [implemented baseline]
