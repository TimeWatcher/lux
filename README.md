<p align="center">
  <img src="images/hero.png" alt="Lux - modern language and GLua toolchain for Garry's Mod addon development" width="100%">
</p>

<h1 align="center">Lux</h1>

<p align="center">
  <strong>A modern language and compiler-first toolchain for Garry's Mod addon development.</strong>
</p>

<p align="center">
  Write expressive Lux source, compile to readable GLua, and let the compiler handle modules, realms, loaders, source maps, diagnostics, packages, and editor intelligence.
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/">Documentation</a>
  ·
  <a href="#quick-start">Quick Start</a>
  ·
  <a href="#syntax-preview">Syntax Preview</a>
  ·
  <a href="#gmod-toolchain">GMod Toolchain</a>
  ·
  <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/"><img src="https://img.shields.io/badge/docs-online-2f6feb" alt="Docs"></a>
  <a href="compiler/"><img src="https://img.shields.io/badge/compiler-Rust-f46623" alt="Rust compiler"></a>
  <a href="https://gmod.facepunch.com/"><img src="https://img.shields.io/badge/target-Garry's%20Mod-1f6feb" alt="Garry's Mod target"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License"></a>
</p>

---

## Why Lux?

GLua is powerful, but real Garry's Mod projects tend to grow the same sharp
edges: globals become accidental API, `include` order becomes folklore, realm
boundaries drift, generated Lua stack traces hide the original source, and
editor tooling has to guess from plain text.

Lux keeps the Lua/GLua feel, but moves project structure into a compiler.

You write Lux. Lux emits ordinary, inspectable GLua.

| GLua pain | Lux answer |
| --- | --- |
| Helpers leak into globals | Modules are private by default; exports are explicit |
| `AddCSLuaFile` and `include` order becomes project lore | Realms are part of the language; GMod loaders are generated |
| Client, server, and shared code are easy to mix incorrectly | `client`, `server`, and `shared` declarations are checked |
| Large addons need more expressive syntax | `fn`, guards, enums, `match`, optional access, `??`, arrows, imports, exports |
| Generated/runtime errors are hard to trace | Source maps map output locations back to Lux source |
| Editor support guesses from loose Lua | `luxc lsp` uses the same parser, resolver, package graph, and realm checker as builds |

## Syntax Preview

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

  detailed then
    `#${index}: ${name} (${player?:Health() ?? 0} hp)`
  else
    name
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

Lux stays close to Lua, but the compiler understands more of your addon:

- `fn` declarations with block or expression bodies
- guard-style exits with `stopif` and `stopifn`
- `enum` and `match` for explicit state
- optional access with `?:` and nil fallback with `??`
- arrow functions for callbacks
- explicit imports and exports
- realm-aware declarations such as `client fn`

## GMod Toolchain

Lux is not a runtime framework. It does not replace Garry's Mod, GLua, or the
APIs you already use.

It is an offline compiler and project toolchain.

```text
Lux source
   |
   v
luxc gmod build
   |
   +- resolves modules and packages
   +- checks client/server/shared realms
   +- generates GMod loader trees
   +- emits readable GLua
   +- writes source maps
   |
   v
generated/lua/
   +- autorun/          optional addon forwarder
   +- lux/<bundle>/     generated loaders and module artifacts
   +- *.lua.map.json    source maps
```

The output is ordinary GLua/Lua 5.1 that can be inspected, debugged, and shipped
with your addon. If an existing gamemode, framework, or hand-written Lua entry
point owns startup, set `autorun = false` or pass `--no-autorun`; Lux still
emits the loader tree.

## Core Features

### Modern Syntax, Lua-Shaped

- functions with `fn`
- block and expression bodies
- guard statements
- arrow callbacks
- optional access
- nil coalescing
- template strings
- destructuring
- table spread
- pipelines
- enums
- checked `match`

### Explicit Modules

Lux modules are private by default. Public API is declared intentionally.

```lux
fn normalizeHealth(hp) =
  hp < 0 then 0 else hp > 100 then 100 else hp

export { normalizeHealth }
```

### Realm-Aware Code

Client, server, and shared ownership is part of the source model.

```lux
shared fn formatName(player) =
  player?:Nick() ?? "unknown"

client fn drawName(player) {
  draw.SimpleText(formatName(player), "DermaDefault", 16, 16)
}

server fn logJoin(player) {
  print(formatName(player) .. " joined")
}
```

Lux can reason about where declarations belong and generate the loader
structure Garry's Mod expects.

### Compiler-Backed Editor Support

`luxc lsp` provides editor support built on the same compiler model used by
builds:

- diagnostics
- hover
- completion
- go to definition
- signature help
- formatting
- semantic tokens
- code actions
- GMod API documentation

The VS Code extension is intentionally thin: it launches the selected compiler
as `luxc lsp`, so editor behavior stays aligned with the Lux version your
project builds with.

### Registryless Packages

Lux has no package registry, mirror source, or global "latest" lookup.
Dependencies point at explicit GitHub, URL, or local path sources. GitHub
sources can be pinned with `tag`, `branch`, or `commit`, and `lux.lock` records
the resolved package graph.

Official standard packages live in
[`TimeWatcher/lux-packages`](https://github.com/TimeWatcher/lux-packages).

## Quick Start

Lux is currently in alpha. No public binary release is active; build `luxc`
from source.

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release
.\target\release\luxc.exe --help
```

Create a project without network access:

```powershell
.\target\release\luxc.exe init ..\my_addon
```

Create a project with the standard package setup:

```powershell
.\target\release\luxc.exe init ..\my_addon --std
```

Install the official GMod package:

```powershell
Push-Location ..\my_addon
..\lux\compiler\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-packages
Pop-Location
```

Build for Garry's Mod:

```powershell
.\target\release\luxc.exe gmod build --manifest ..\my_addon\lux.toml
```

Minimal manifest:

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
thin addon-style forwarder under `out/autorun`.

## When To Use Lux

Lux is a good fit for:

- new Garry's Mod addons
- gamemodes with growing client/server/shared structure
- addons that need private modules and explicit public APIs
- projects that want better editor diagnostics
- gradual migrations beside existing GLua
- codebases where loader order has become hard to reason about

Lux is probably not necessary for:

- tiny one-file scripts
- throwaway test snippets
- addons where plain GLua is already sufficient

## Status

Lux is alpha software. The language, package layout, LSP integration, and GMod
backend are usable for experiments and migration work, but breaking changes are
expected while the toolchain stabilizes.

What works today:

- module directories with multi-part lexical scope
- `client`, `server`, and `shared` declarations and blocks
- explicit `import` / `export` APIs with realm-aware validation
- generated GMod loader trees with optional `autorun` forwarders
- source maps and source comments for generated Lua
- dependency sources from GitHub, URL, or local paths
- `luxc install`, `luxc lock`, `luxc remove`, `luxc doctor`, and `lux.lock`
- `luxc lsp` for editor support
- official GMod API data shared by compiler checks and editor intelligence

## Documentation

- [Getting started](https://timewatcher.github.io/lux-docs-site/guide/getting-started)
- [Language overview](https://timewatcher.github.io/lux-docs-site/language/)
- [Modules and parts](https://timewatcher.github.io/lux-docs-site/language/modules)
- [Imports and exports](https://timewatcher.github.io/lux-docs-site/language/imports-exports)
- [Realms](https://timewatcher.github.io/lux-docs-site/language/realms)
- [Packages](https://timewatcher.github.io/lux-docs-site/packages/)
- [GMod backend](https://timewatcher.github.io/lux-docs-site/gmod/)
- [VS Code and LSP](https://timewatcher.github.io/lux-docs-site/reference/vscode)
- [Standard packages](https://github.com/TimeWatcher/lux-packages)
- [LSP and VS Code](https://github.com/TimeWatcher/lux-lsp)
- [MGFX](https://github.com/TimeWatcher/lux-mgfx)

## Repository Map

```text
compiler/        Rust implementation of luxc, including luxc lsp
lsp/             VS Code shell and shared GMod API intelligence data
docs-site/       Public Lux documentation site, tracked as a submodule
docs/            Design notes and implementation references
examples/        Lux and GMod example projects
images/          README and project media assets
```

Initialize optional submodules when working on LSP or the documentation site:

```powershell
git submodule update --init lsp docs-site
```

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
