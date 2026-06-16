<div align="center">

# Lux

**A compiler-first language for Garry's Mod addons that need structure without
giving up readable GLua.**

[![Docs](https://img.shields.io/badge/docs-online-2f6feb)](https://timewatcher.github.io/lux-docs-site/)
[![Rust](https://img.shields.io/badge/compiler-Rust-f46623)](compiler/)
[![Garry's Mod](https://img.shields.io/badge/target-Garry's%20Mod-1f6feb)](https://gmod.facepunch.com/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

[Documentation](https://timewatcher.github.io/lux-docs-site/) ·
[Quick Start](#quick-start) ·
[Standard Packages](https://github.com/TimeWatcher/lux-std) ·
[VS Code](https://github.com/TimeWatcher/lux-lsp) ·
[MGFX](https://github.com/TimeWatcher/lux-mgfx)

English · [简体中文](README.zh-CN.md)

</div>

Lux is a Lua-oriented language and toolchain for Garry's Mod. It compiles
offline to ordinary GLua/Lua 5.1, keeps generated output inspectable, and moves
the fragile parts of real addon development into compiler-owned checks:
modules, realm loading, imports, exports, source maps, package resolution, and
editor diagnostics.

Lux is not a framework that takes over your addon. It is a compiler you can use
for a new addon, a gamemode, or a gradual migration beside existing GLua.

## Why Use It

| GLua pain | Lux answer |
| --- | --- |
| Private helpers leak into globals. | Directory modules are private by default and export only explicit names. |
| `AddCSLuaFile` and `include` order become project lore. | Realms are part of the language, and GMod loaders are generated from the module graph. |
| Large addons need structure but still need the GMod API. | Lux emits readable Lua and lets ordinary GMod/third-party calls pass through. |
| Editor tooling guesses from text. | `luxc lsp` uses the same parser, resolver, package graph, and realm checker as builds. |
| Standard code should not be pinned to compiler releases. | Official packages live in `lux-std` and are locked by the project. |

## What Works Today

- module directories with multi-part lexical scope
- `client`, `server`, and `shared` declarations and blocks
- explicit `import` / `export` APIs with realm-aware validation
- generated GMod loader trees with optional `autorun` forwarders
- source maps and source comments for generated Lua
- no registry package model: dependencies point at explicit GitHub, URL, or path sources
- `luxc install`, `luxc lock`, `luxc remove`, `luxc doctor`, and `lux.lock`
- `luxc lsp` for VS Code hover, completion, definition, signature help, diagnostics, formatting, and GMod API docs
- official GMod API database shared by compiler checks and editor intelligence

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

The syntax stays close to Lua, but modules are private, public API is explicit,
realm ownership is checked, nil-heavy GMod calls are easier to write, and the
same compiler model powers the editor.

## Quick Start

No public binary release is currently active. Build `luxc` from source during
the alpha:

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release
.\target\release\luxc.exe --help
```

Initialize a project without network access:

```powershell
.\target\release\luxc.exe init .\my_addon
```

Install official packages only when you ask for them:

```powershell
.\target\release\luxc.exe init .\my_addon --std
.\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-std --project .\my_addon
```

Lux has no package registry. A dependency's source and version are selected by
the explicit `github`, `url`, or `path` entry in `lux.toml`, plus optional
`tag`, `branch`, or `commit` refs. `lux.lock` records the resolved package set.
`luxc lock` regenerates the lockfile from the manifest; it does not search for
newer versions. `luxc remove` removes a direct dependency and prunes unused
transitive packages.

Build a GMod addon project:

```powershell
.\target\release\luxc.exe gmod build --manifest .\lux.toml
```

Use `--no-autorun` or `autorun = false` when an existing gamemode, framework,
or hand-written Lua entry point will include the generated Lux loaders itself.
The loader tree is still emitted.

## Minimal Manifest

```toml
package_id = "my_addon"
bundle_id = "my_addon"

[target.gmod]
source_root = "src"
out = "generated/lua"
runtime_base = "lux/my-addon"
autorun = true
source_comments = "boundary"

[dependencies]
```

`out` is the physical output root. `runtime_base` is the GMod-relative path used
inside generated `include` and `AddCSLuaFile` calls. `autorun` controls only the
thin addon-style forwarder under `out/autorun`; it does not disable the Lux
loader tree.

## Build From Source

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo test
cargo build --release
```

The compiler binary is written to:

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
compiler/        Rust implementation of luxc, including luxc lsp
lsp/             VS Code shell and shared GMod API intelligence data
docs-site/       Public Lux documentation site, tracked as a submodule
docs/            Design notes and implementation references
examples/        Lux and GMod example projects
```

Initialize optional submodules when working on LSP or the documentation site:

```powershell
git submodule update --init lsp docs-site
```

## CLI Snapshot

```text
luxc lex <path>
luxc parse <path>
luxc lint <path>
luxc format <path> [--check] [--write]
luxc init [path] [--name <name>] [--std] [--out <path>] [--runtime-base <path>] [--no-autorun]
luxc install <package-id> --from <github:owner/repo|url|path> [--tag <tag>|--branch <branch>|--commit <commit>]
luxc remove <package-id> [--project <project-root>]
luxc lock [project-root]
luxc list [project-root]
luxc doctor [project-root]
luxc lsp
luxc compile <path> [--map <path>] [--source-comments [none|readable|boundary|dense]]
luxc map-error <map.json> <generated-line>
luxc gmod build <source-root> --out <path> [--runtime-base <path>] [--no-autorun] [--dry-run]
luxc gmod build --manifest <lux.toml> [--out <path>] [--runtime-base <path>] [--no-autorun] [--dry-run]
luxc gmod package --manifest <lux.toml> --root <path> --gmad <path> --out <path> [--run] [--build-out <path>] [--runtime-base <path>] [--no-autorun]
luxc gmod api update [--out <path>] [--coverage-out <path>] [--cache-dir <path>] [--offline] [--allow-failures]
```

## Documentation

- [Getting started](https://timewatcher.github.io/lux-docs-site/guide/getting-started)
- [Language overview](https://timewatcher.github.io/lux-docs-site/language/)
- [Modules and parts](https://timewatcher.github.io/lux-docs-site/language/modules)
- [Imports and exports](https://timewatcher.github.io/lux-docs-site/language/imports-exports)
- [Realms](https://timewatcher.github.io/lux-docs-site/language/realms)
- [Packages](https://timewatcher.github.io/lux-docs-site/packages/)
- [GMod backend](https://timewatcher.github.io/lux-docs-site/gmod/)
- [VS Code and LSP](https://timewatcher.github.io/lux-docs-site/reference/vscode)
- [LSP repository](https://github.com/TimeWatcher/lux-lsp)
- [MGFX repository](https://github.com/TimeWatcher/lux-mgfx)

## Status

Lux is alpha software without an active public binary release. The language,
package layout, LSP integration, and GMod backend are usable for experiments and
migration work, but the project should still be treated as pre-1.0.

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

Official standard packages live in the separate
[`lux-std`](https://github.com/TimeWatcher/lux-std) repository. Edit that
repository directly and validate with compiler tests or a GMod project build
that imports the changed package.

VS Code support and GMod API intelligence data live in the `lsp` submodule. The
language server itself is provided by `luxc lsp`; edit the compiler when
changing hover, completion, signature help, diagnostics, or quick fixes. The
LSP uses the compiler's package resolution and module analysis, so cross-part
and imported definitions stay aligned with the selected `luxc`.

## License

Lux uses a split license model:

- Source code is licensed under `MIT OR Apache-2.0`, except for separately
  licensed packages.
- Documentation prose is licensed under `CC-BY-4.0`.
- Code examples in documentation are licensed under `MIT OR Apache-2.0`.
- The Lux name, logo, icon, and other branding assets are not licensed for
  reuse by these open source licenses.

Using `luxc` to compile your source code does not change the license of your
addon or generated project. If generated Lua embeds Lux runtime or package code,
that embedded Lux code keeps its package license.

See [LICENSE](LICENSE), [LICENSE-MIT](LICENSE-MIT),
[LICENSE-APACHE](LICENSE-APACHE), [LICENSE-DOCS](LICENSE-DOCS), and
[NOTICE](NOTICE).
