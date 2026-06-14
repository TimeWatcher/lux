# Lux

Lux is a compiler-first language and toolchain for Garry's Mod Lua development.
It keeps the output close to GLua/Lua 5.1 while adding a modern module system,
realm-aware project builds, explicit public APIs, macros, host transforms, source
maps, and practical syntax improvements for everyday addon work.

Lux is not a runtime framework that takes over your addon. The compiler runs
offline, emits ordinary Lua files, and generates the GMod loader code needed to
mount client, server, and shared modules correctly.

## Status

Lux is currently an early `0.1.0` compiler release. The language, package
layout, and GMod backend are usable for experimentation and migration work, but
the project should still be treated as pre-1.0.

- Documentation: <https://timewatcher.github.io/lux-docs-site/>
- Release builds: <https://github.com/TimeWatcher/lux/releases>
- Built-in packages: <https://github.com/TimeWatcher/lux-packages>
- Documentation source: <https://github.com/TimeWatcher/lux-docs-site>

## Why Lux

GMod addon code usually has to solve three problems at the same time:

- Lua lacks a real module boundary, so private code and public API blur together.
- GMod realm loading requires boilerplate such as `if SERVER then AddCSLuaFile(...) end`.
- Large addons need compile-time checks, stable generated output, and readable
  diagnostics without giving up normal GLua interoperability.

Lux targets those problems directly:

- **Directory modules**: a module is a directory of part files sharing one
  logical module scope.
- **Explicit exports**: module internals stay private unless exported by name.
- **Realm-aware code**: `client`, `server`, `shared`, and realm blocks are part
  of the language model.
- **Convention packages**: runtime, macro, compile-time, and host code are
  discovered by directory layout instead of per-package manifests.
- **Readable Lua output**: generated GLua remains inspectable and source-map
  aware.

## Install

Download the latest Windows build from the release page:

<https://github.com/TimeWatcher/lux/releases/tag/v0.1.0>

Unzip it and keep the bundled `packages` directory next to `luxc.exe`:

```text
luxc-v0.1.0-x86_64-pc-windows-msvc/
  luxc.exe
  packages/
```

Then run:

```powershell
.\luxc.exe --help
```

If you want to use a different package root, set `LUX_PACKAGE_ROOT`:

```powershell
$env:LUX_PACKAGE_ROOT = "C:\path\to\lux-packages"
.\luxc.exe compile .\src\module.lux
```

## Build From Source

Clone with submodules:

```powershell
git clone --recurse-submodules https://github.com/TimeWatcher/lux.git
cd lux
```

Build and test the compiler:

```powershell
cd compiler
cargo test
cargo build --release
```

The compiler binary will be written to:

```text
compiler/target/release/luxc.exe
```

## Quick Start

Compile one Lux file to Lua:

```powershell
.\compiler\target\release\luxc.exe compile .\examples\features.lux
```

Build a GMod addon project:

```powershell
.\compiler\target\release\luxc.exe gmod build --manifest .\examples\gmod_project\lux.toml
```

A minimal GMod manifest looks like this:

```toml
[gmod]
source_root = "src"
addon_root = "generated"
package_id = "my_addon"
bundle_id = "my_addon"

[target.gmod.realm]
unknown_external = "warn"
```

## Language Shape

Lux is designed as a Lua-oriented language, not as a replacement for the GMod
API. Normal GLua calls stay recognizable, while Lux adds compile-time structure
around them.

```lux
import { color, valid } from "@lux/gmod"

fn displayName(ply) =
  ply?:Nick() ?? "Unknown"

server fn spawnCrate(pos) {
  local ent = ents.Create("prop_physics")
  ent:SetPos(pos)
  ent:Spawn()
  ent
}

client fn drawName(ply) {
  stopifn valid.is(ply)

  draw.SimpleText(
    displayName(ply),
    "DermaDefault",
    12,
    12,
    color.white()
  )
}

export client { drawName }
export server { spawnCrate }
```

Key concepts:

- Top-level declarations are module-private by default.
- `export { public_name = local_binding }` maps internal bindings to public API
  names.
- Imports bind to exported API names, not filenames.
- Shared modules can contain client-only and server-only declarations when those
  declarations are explicitly marked.
- `server { ... }`, `client { ... }`, and `shared { ... }` blocks express
  fine-grained realm-specific code.

## Repository Layout

```text
compiler/   Rust implementation of luxc
packages/   Built-in Lux packages, tracked as a submodule
docs-site/  Public documentation site, tracked as a submodule
docs/       Design notes and implementation references
examples/   Small Lux and GMod project examples
```

The `packages` and `docs-site` directories are independent repositories. Use
`--recurse-submodules` when cloning, or run this after cloning:

```powershell
git submodule update --init --recursive
```

## CLI

```text
luxc lex <path>
luxc parse <path>
luxc lint <path>
luxc format <path> [--check] [--write]
luxc compile <path> [--map <path>] [--source-comments [none|readable|boundary|dense]]
luxc map-error <map.json> <generated-line>
luxc gmod build <source-root> <addon-root> [--generated-root <path>] [--dry-run]
luxc gmod build --manifest <lux.toml> [--generated-root <path>] [--dry-run]
luxc gmod package --manifest <lux.toml> --gmad <path> --out <path> [--run] [--generated-root <path>]
```

Common development commands:

```powershell
cd compiler
cargo test
cargo run -- compile ..\examples\features.lux
cargo run -- gmod build --manifest ..\examples\gmod_project\lux.toml --dry-run
```

## Documentation

Start here:

- Getting started: <https://timewatcher.github.io/lux-docs-site/guide/getting-started>
- Language overview: <https://timewatcher.github.io/lux-docs-site/language/>
- Modules and parts: <https://timewatcher.github.io/lux-docs-site/language/modules>
- Imports and exports: <https://timewatcher.github.io/lux-docs-site/language/imports-exports>
- Realms: <https://timewatcher.github.io/lux-docs-site/language/realms>
- GMod backend: <https://timewatcher.github.io/lux-docs-site/gmod/>
- Generated Lua: <https://timewatcher.github.io/lux-docs-site/reference/generated-lua>

Chinese documentation is available under:

<https://timewatcher.github.io/lux-docs-site/zh/>

## Contributing

For compiler work:

```powershell
cd compiler
cargo test
```

For documentation work:

```powershell
cd docs-site
npm install
npm run dev -- --host 127.0.0.1 --port 4173
npm run build
```

For package work, edit the `packages` submodule and run compiler tests or a
GMod project build that imports the package being changed.

## License

No license file has been added yet. Treat the repository as source-available
until a license is chosen.
