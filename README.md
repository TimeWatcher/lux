<div align="center">

# Lux

**A compiler-first language for building Garry's Mod addons without loader
spaghetti.**

[![Release](https://img.shields.io/github/v/release/TimeWatcher/lux?label=release)](https://github.com/TimeWatcher/lux/releases)
[![Docs](https://img.shields.io/badge/docs-online-2f6feb)](https://timewatcher.github.io/lux-docs-site/)
[![Rust](https://img.shields.io/badge/compiler-Rust-f46623)](compiler/)
[![Garry's Mod](https://img.shields.io/badge/target-Garry's%20Mod-1f6feb)](https://gmod.facepunch.com/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

[Documentation](https://timewatcher.github.io/lux-docs-site/) ·
[Quick Start](#quick-start) ·
[Packages](https://github.com/TimeWatcher/lux-packages) ·
[LSP](https://github.com/TimeWatcher/lux-lsp) ·
[MGFX](https://timewatcher.github.io/mgfx-docs-site/) ·
[Releases](https://github.com/TimeWatcher/lux/releases)

English · [简体中文](README.zh-CN.md)

</div>

Lux is a Lua-oriented language and toolchain for Garry's Mod. It compiles
offline to ordinary GLua/Lua 5.1, keeps the generated output inspectable, and
lets the compiler own the parts that are usually fragile in real addons:
module boundaries, realm loading, package discovery, exports, source maps, and
diagnostics.

Lux is not a runtime framework that takes over your addon. It is a compiler
that helps you write clearer GLua projects and emits the loader code Garry's Mod
expects.

## Why Lux

GMod addon code tends to grow around three pain points:

| Problem in GLua projects | Lux answer |
| --- | --- |
| Private helpers become accidental global API. | Directory modules are private by default and export only explicit names. |
| Realm loading turns into `AddCSLuaFile` boilerplate and fragile include order. | `client`, `server`, `shared`, realm blocks, and generated GMod loaders are first-class. |
| Large addons need structure without losing GLua interoperability. | Lux compiles to readable Lua and still allows normal GMod and third-party API calls. |

## What It Feels Like

```lux
extern client drawHud

import { arr } from "@lux/std"
import { hookx, valid } from "@lux/gmod"

enum HudMode repr string {
  Compact = "compact",
  Detailed = "detailed"
}

fn title(mode) =
  match mode {
    HudMode.Compact => "HUD"
    HudMode.Detailed => "Detailed HUD"
  }

fn playerLine(player, index, detailed) {
  stopifn valid.is(player), `#${index}: missing`

  local name = player?:Nick() ?? "unknown"
  detailed then `#${index}: ${name} (${player?:Health() ?? 0} hp)` else name
}

client fn paintHud(players, mode = HudMode.Compact) {
  local detailed = mode == HudMode.Detailed
  local lines = arr.map(players, (player, index) =>
    playerLine(player, index, detailed)
  )

  hookx.add("HUDPaint", "LuxHud", () => drawHud(title(mode), lines))
}

export client { paintHud }
```

This is still close to Lua, but with module-private declarations, explicit
realm ownership, enums and `match`, expression returns, arrow callbacks,
optional access, nil coalescing, and exports that describe the real public API.

## Highlights

- **Module directories, not manifest noise**: a module is a directory of part
  files with one logical module-private scope.
- **Explicit public interfaces**: `export { public_name = local_binding }`
  maps internal names to API names without exposing everything else.
- **Realm-aware by construction**: `client fn`, `server fn`, `shared` code, and
  `client { ... }` / `server { ... }` blocks model GMod execution directly.
- **Smart GMod output**: generated loaders batch client files, avoid global
  filename collisions, and keep source-map/debug information available.
- **Practical syntax**: `match`, `then/else`, arrow functions, optional calls,
  destructuring, table spread, pipeline helpers, and implicit expression
  returns.
- **Packages by convention**: runtime, compile-time, macro, and host code are
  discovered from directory layout instead of handwritten package manifests.

## Quick Start

Download the latest Windows build from
[Releases](https://github.com/TimeWatcher/lux/releases), unzip it, and keep the
bundled `packages` directory next to `luxc.exe`:

```text
luxc-v0.1.0-x86_64-pc-windows-msvc/
  luxc.exe
  packages/
```

Run:

```powershell
.\luxc.exe --help
.\luxc.exe compile .\src\module.lux
```

Build a GMod addon project:

```powershell
.\luxc.exe gmod build --manifest .\lux.toml
```

A small GMod manifest:

```toml
[gmod]
source_root = "src"
addon_root = "generated"
package_id = "my_addon"
bundle_id = "my_addon"

[target.gmod.realm]
unknown_external = "warn"
```

## Build From Source

```powershell
git clone --recurse-submodules https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo test
cargo build --release
```

The compiler binary will be written to:

```text
compiler/target/release/luxc.exe
```

Useful development commands:

```powershell
cargo run -- compile ..\examples\features.lux
cargo run -- gmod build --manifest ..\examples\gmod_project\lux.toml --dry-run
```

## Repository Map

```text
compiler/        Rust implementation of luxc
packages/        Built-in Lux packages, tracked as a submodule
lsp/             Lux LSP, VS Code support, and GMod API intelligence standards
docs-site/       Public Lux documentation site, tracked as a submodule
mgfx-docs-site/  MGFX documentation site, tracked as a submodule
docs/            Design notes and implementation references
examples/        Small Lux and GMod project examples
```

After cloning without submodules:

```powershell
git submodule update --init --recursive
```

## CLI Snapshot

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
luxc gmod api update [--out <path>] [--coverage-out <path>] [--cache-dir <path>] [--override <json>]
```

## Documentation

- [Getting started](https://timewatcher.github.io/lux-docs-site/guide/getting-started)
- [Language overview](https://timewatcher.github.io/lux-docs-site/language/)
- [Modules and parts](https://timewatcher.github.io/lux-docs-site/language/modules)
- [Imports and exports](https://timewatcher.github.io/lux-docs-site/language/imports-exports)
- [Realms](https://timewatcher.github.io/lux-docs-site/language/realms)
- [GMod backend](https://timewatcher.github.io/lux-docs-site/gmod/)
- [LSP and VS Code standards](https://github.com/TimeWatcher/lux-lsp)
- [MGFX package docs](https://timewatcher.github.io/mgfx-docs-site/)

## Status

Lux is currently an early `0.1.0` compiler release. The language, package
layout, and GMod backend are usable for experimentation and migration work, but
the project should still be treated as pre-1.0.

## Contributing

Compiler:

```powershell
cd compiler
cargo test
```

Documentation:

```powershell
cd docs-site
npm install
npm run dev -- --host 127.0.0.1 --port 4173
npm run build
```

MGFX documentation:

```powershell
cd mgfx-docs-site
npm install
npm run dev -- --host 127.0.0.1 --port 4174
npm run build
```

Packages live in the `packages` submodule. Edit that repository directly and
validate with compiler tests or a GMod project build that imports the changed
package.

Language server and VS Code support standards live in the `lsp` submodule. Edit
that repository directly when working on editor integration, GMod API
intelligence, hover, completion, diagnostics, or quick fixes.

## License

Lux uses a split license model:

- Source code is licensed under `MIT OR Apache-2.0`, except for separately
  licensed packages.
- The bundled `@lux/mgfx` package is licensed for non-commercial use only.
  Commercial use of MGFX requires a separate written license from the copyright
  holder.
- Documentation prose is licensed under `CC-BY-4.0`.
- Code examples in documentation are licensed under `MIT OR Apache-2.0`.
- The Lux name, logo, icon, and other branding assets are not licensed for
  reuse by these open source licenses.

Using `luxc` to compile your source code does not change the license of your
addon or generated project. If generated Lua embeds Lux runtime or package code,
that embedded Lux code keeps its package license: most Lux code remains
`MIT OR Apache-2.0`, while embedded MGFX code remains under the Lux MGFX
Non-Commercial License.

See [LICENSE](LICENSE), [LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE), [LICENSE-DOCS](LICENSE-DOCS), and
[NOTICE](NOTICE). For MGFX, see `packages/LICENSE-MGFX-NC`.
