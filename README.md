# Lux

Lux is a new Lua-first language and toolchain for modern GLua development.

The project goal is not "UI only". UI is an optional host layer built on top of
the language, standard library, reactive core, and compiler infrastructure.

## Direction

- Lua superset, not a brand new language
- Offline compiler only
- Pure GLua/Lua 5.1 output
- Modern authoring: declarative, functional, composable, reactive
- UI as an optional enhancement, not the whole identity of the project

## Layout

- `docs/` - language, architecture, backend, and roadmap notes
- `compiler/` - current Rust `luxc` implementation
- `packages/` - Lux packages using convention-based runtime, macro, and host phases
- `examples/` - sample Lux source files

## Current Status

Lux now has an initial compiler pipeline:

```text
source -> lexer -> parser -> macro expansion -> resolver -> normalized IR -> host transforms -> LuaWriter + SourceMap
```

Macros, host transforms, and runtime libraries are loaded from `packages/`
through directory conventions: every `.lux` file under `src/`, `compiletime/`,
or `host/` becomes a part of that package phase. Rust provides the compiler ABI
and structured AST/IR builders; package-specific behavior such as `lux/ui`
folding is not embedded in codegen.

Developer tooling is also starting to come online:

- `luxc lint <file>` reports semantic footguns such as newline tail-table calls
  and semicolon-suppressed implicit returns.
- `luxc format <file> [--check|--write]` is a conservative lossless formatter:
  it only rewrites whitespace/indentation and verifies token text, comment
  text, and parsed AST shape before writing. Write-back uses a same-directory
  temporary file and preserves the original source on formatter safety errors.
- `luxc map-error <map.json> <generated-line>` maps generated Lua stack lines
  back to Lux source locations.

The GMod backend also has a realm-aware build plan model for generated Lua
paths, loader operations, private module registry glue, and `.gma` command
planning. `.gma` packaging is optional and explicit, intended to reduce client
Lua download/mount overhead when useful. It does not publish or mutate a live
addon as a side effect of normal compilation.

Start with:

- `docs/LANGUAGE_MVP_0.1.md`
- `docs/TOKENS.md`
- `docs/PRECEDENCE.md`
- `docs/AST.md`
- `docs/ARCHITECTURE.md`
- `docs/GLUA_BACKEND.md`
- `docs/GMOD_BACKEND.md`
- `docs/STDLIB.md`
- `docs/COMPILER_IMPLEMENTATION.md`
- `docs/ROADMAP.md`
