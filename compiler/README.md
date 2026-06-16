# compiler

This directory holds the current `luxc` implementation.

Current responsibilities:

- lexer
- parser
- AST
- scope / import / export resolver
- offline module graph resolution
- normalized IR lowering
- Lua/GLua code generation through a source-map-aware writer
- sidecar source map emission
- GMod project artifact and loader generation
- compiler-backed Language Server Protocol entrypoint
- CLI entrypoints

Planned responsibilities:

- host plugin integration
- project manifest loading
- watch mode
- GMA packaging execution as an explicit command

## Commands

```powershell
cargo run -- lex path\to\file.lux
cargo run -- parse path\to\file.lux
cargo run -- compile path\to\file.lux
cargo run -- compile path\to\file.lux --map path\to\file.lua.map.json
cargo run -- build path\to\src --out path\to\generated\lua
cargo run -- build path\to\src --out path\to\generated\lua --map
cargo run -- gmod build path\to\src --out path\to\generated\lua
cargo run -- gmod build path\to\src --out path\to\generated\lua --dry-run
cargo run -- gmod build path\to\src --out path\to\generated\lua --no-autorun
cargo run -- gmod build --manifest path\to\lux.toml
cargo run -- self install --default
cargo run -- self list
cargo run -- self pin 0.1.0-alpha.4 --project path\to\project
cargo run -- lsp
```

`compile` prints generated Lua to stdout. `--map` writes a JSON sidecar source
map while keeping Lua output on stdout.

`build` recursively compiles every `.lux` file under a source directory as an
independent single-file compile, preserves the source-relative directory
structure, and writes matching `.lua` files under `--out`. It does not generate
GMod loaders, realm dispatch files, or an `autorun` forwarder.

`gmod build` compiles every `.lux` file under the source root, resolves imports
offline, emits wrapped Lua modules, source-map sidecars, the Lux loader tree
under `out/runtime_base`, and an optional `autorun` forwarder.

`lsp` starts the compiler-owned Language Server Protocol process. Editors should
launch the same `luxc` selected for project builds so diagnostics, completion,
hover, go-to-definition, signature help, package resolution, and emitted Lua
semantics stay version-aligned. Cross-part and imported Lux symbols are resolved
through the same analysis graph used by builds.

`self` manages compiler toolchains separately from Lux source packages. A normal
developer installs one stable entrypoint at `~/.lux/bin/luxc`, and that shim
dispatches to `~/.lux/toolchains/<version>/luxc` using either the global default
or an optional project pin at `.lux/toolchain.toml`. Single-file use and ordinary
projects do not need a pin.

The GMod build path also injects required compiler-provided runtime modules.
Currently `import { arr } from "@lux/std"` adds a generated `lux/std` module with
basic array helpers before project modules are loaded.

Minimal manifest:

```toml
[target.gmod]
source_root = "src"
out = "generated/lua"
runtime_base = "lux/my-addon"
autorun = true
```

`out` is the physical output root. `runtime_base` is the GMod relative path used
by generated `include` and `AddCSLuaFile` calls. Set `autorun = false` when an
existing gamemode, framework, or hand-written entry point will include the Lux
loaders itself.

Implementation blueprint:

- `../docs/COMPILER_IMPLEMENTATION.md`

Recommended MVP shape:

- one Rust crate first
- hand-written lexer
- recursive-descent parser with Pratt expression parsing
- explicit resolver pass for scope/import bindings
- normalized IR before Lua codegen
- host transforms after resolution, before final emission
