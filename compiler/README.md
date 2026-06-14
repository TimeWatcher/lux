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
cargo run -- gmod build path\to\src path\to\addon
cargo run -- gmod build path\to\src path\to\addon --dry-run
cargo run -- gmod build path\to\src path\to\addon --generated-root path\to\generated
cargo run -- gmod build --manifest path\to\lux.toml
```

`compile` prints generated Lua to stdout. `--map` writes a JSON sidecar source
map while keeping Lua output on stdout.

`gmod build` compiles every `.lux` file under the source root, resolves imports
offline, emits wrapped Lua modules, source-map sidecars, and three GMod loader
files under the addon/generated root.

The GMod build path also injects required compiler-provided runtime modules.
Currently `import { arr } from "@lux/std"` adds a generated `lux/std` module with
basic array helpers before project modules are loaded.

Minimal manifest:

```toml
[gmod]
source_root = "src"
addon_root = "."
generated_root = "generated"
```

`generated_root` is optional and defaults to `addon_root`.

Implementation blueprint:

- `../docs/COMPILER_IMPLEMENTATION.md`

Recommended MVP shape:

- one Rust crate first
- hand-written lexer
- recursive-descent parser with Pratt expression parsing
- explicit resolver pass for scope/import bindings
- normalized IR before Lua codegen
- host transforms after resolution, before final emission
