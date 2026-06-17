<p align="center">
  <img src="images/hero.png" alt="Lux - compiler-first language and GLua toolchain for Garry's Mod" width="100%">
</p>

<h1 align="center">Lux</h1>

<p align="center">
  <strong>Better GLua syntax when you only need one file. Compiler-owned structure when your Garry's Mod project grows.</strong>
</p>

<p align="center">
  Lux is an open-source language layer and toolchain for Garry's Mod. It compiles offline to ordinary readable GLua / Lua 5.1 while adding nil-safe expressions, real modules, client/server/shared ownership, generated GMod loaders, source maps, registryless packages, and compiler-backed editor diagnostics.
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/">Documentation</a>
  ·
  <a href="#quick-start">Quick Start</a>
  ·
  <a href="#one-file">One File</a>
  ·
  <a href="#gmod-projects">GMod Projects</a>
  ·
  <a href="#packages">Packages</a>
  ·
  <a href="#mgfx">MGFX</a>
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

## What Lux Is

Lux is not a runtime framework and it does not replace Garry's Mod, GLua, or the
APIs you already use.

It is a compiler-first source layer:

```text
Lux source
  -> luxc
  -> readable GLua / Lua 5.1
  -> normal Garry's Mod files
```

You can use Lux in two sizes:

```text
one Lux file
  -> safer, more expressive GLua-shaped syntax
  -> printed as ordinary Lua

GMod project
  -> modules, imports, exports, realms, packages
  -> generated loader tree and source maps
  -> compiler-backed LSP diagnostics
```

The output remains inspectable Lua. Existing GLua, Facepunch APIs, third-party
libraries, gamemodes, and hand-written entry points can still own runtime
behavior.

## What Lux Fixes

Real GMod code tends to grow the same structural problems. Lux moves those
rules into the language and compiler instead of leaving them as project lore.

| In GLua projects | With Lux |
| --- | --- |
| Helpers quietly become globals | Modules are private by default; public API is explicit |
| `include` order and `AddCSLuaFile` calls become fragile | The compiler builds the loader tree |
| Client, server, and shared ownership drifts over time | `client`, `server`, and `shared` are checked source declarations |
| Optional player/entity/UI state creates nil crashes | `?:` and `??` express optional data directly |
| `condition and a or b` breaks when `a` is `false` | `then ... else ...` is a real conditional expression |
| Generated Lua errors are hard to map back | Source maps and source comments preserve source intent |
| Editor support guesses from loose text | `luxc lsp` uses the compiler parser, resolver, package graph, and realm model |

## Syntax That Pays Rent

Lux stays close to Lua, but adds constructs that match common GLua patterns.

### Real Conditional Expressions

Lua's pseudo-ternary pattern is not safe when the middle value can be `false`:

```lua
local enabled = shouldEnable() and false or true
-- enabled becomes true
```

Lux makes the branch explicit:

```lux
local enabled = shouldEnable() then false else true
```

### Nil-Only Fallback

Use `??` when only `nil` should fall back. `false` remains a real value.

```lux
local title = panelTitle ?? "Untitled"
local visible = config.visible ?? true
```

### Nil-Safe Access

Optional data access stays visible without turning every line into nested
checks.

```lux
local name = player?:Nick() ?? "unknown"
local owner = weapon?:GetOwner()?:Nick() ?? "no owner"
```

This does not replace `IsValid` checks. It prevents ordinary nil-indexing bugs
when data is genuinely optional.

### Guards And Callbacks

```lux
stopifn valid.is(player)
stopifn data.items

arr.map(players, (player, index) => playerLine(player, index))
```

Early exits and small callbacks stay proportional to the work they do.

### Enum And Match

```lux
enum HudMode repr string {
  Compact = "compact",
  Detailed = "detailed"
}

fn title(mode) =
  match mode {
    HudMode.Compact => "HUD"
    HudMode.Detailed => "Detailed HUD"
  }
```

State-heavy HUDs, weapons, entities, UI routes, network messages, and parsers
can keep state names and state behavior together.

## What It Looks Like

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

The compiler understands functions, guards, enums, match expressions, optional
access, nil fallback, callbacks, imports, exports, and client/server/shared
ownership.

## One File

Lux can be used as a single-file syntax upgrade. You do not need a package
graph, a generated addon layout, or an autorun entry point just to get the
language improvements.

```lux
fn linesFor(players) {
  local lines = {}

  for i = 1, #players {
    local player = players[i]
    lines[#lines + 1] = `#${i}: ${player?:Nick() ?? "unknown"}`
  }

  lines
}

fn paintHud(players) {
  hook.Add("HUDPaint", "ExampleHud", () => drawHud(linesFor(players)))
}
```

Single-file compilation prints ordinary Lua:

```powershell
.\target\release\luxc.exe compile .\hud.lux
```

Use this mode for small scripts, experiments, generated snippets, or gradual
migration beside existing GLua.

## GMod Projects

When the source tree grows, Lux can own the project structure that is usually
spread across folder conventions and handwritten loader glue.

Project mode adds:

- explicit imports and exports
- private modules by default
- multi-part module scope
- `client`, `server`, and `shared` declarations
- realm-aware validation
- generated GMod loader trees
- optional addon-style `autorun` forwarders
- source maps and source comments
- package resolution
- compiler-backed LSP diagnostics

Instead of maintaining loader order by hand:

```lua
if SERVER then
  AddCSLuaFile("cl_hud.lua")
  AddCSLuaFile("shared/state.lua")
  include("shared/state.lua")
  include("sv_data.lua")
end

if CLIENT then
  include("shared/state.lua")
  include("cl_hud.lua")
end
```

you write ownership in source:

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

and Lux generates the GMod-facing output.

## GMod Output Model

The default project shape is addon-oriented: `luxc init` writes `autorun = true`.
That means Lux emits a thin `autorun` forwarder that includes the generated
loaders.

```text
generated/lua/
  autorun/
    my_addon.lua
  lux/
    my_addon/
      loader_shared.lua
      loader_client.lua
      loader_server.lua
      ...
      *.lua.map.json
```

`--no-autorun` or `autorun = false` only disables that thin forwarder. It does
not mean "gamemode mode", and it does not disable the generated loader tree.
Use it when an existing gamemode, framework, or hand-written Lua entry point
will include the Lux loaders itself.

The two important paths are:

- `out`: physical output root on disk, usually `generated/lua`
- `runtime_base`: GMod-relative base path used in generated `include` and `AddCSLuaFile` calls

This keeps generated includes relative and explicit instead of assuming every
project is laid out the same way.

Minimal manifest:

```toml
package_id = "my_addon"
bundle_id = "my_addon"

[target.gmod]
source_root = "src"
out = "generated/lua"
runtime_base = "lux/my_addon"
autorun = true
source_comments = "boundary"

[dependencies]
```

## Packages

Lux has no package registry, mirror source, or global "latest" lookup.
Dependencies point at explicit sources:

- GitHub repository
- URL
- local path

GitHub sources can be pinned with `tag`, `branch`, or `commit`, and `lux.lock`
records the resolved package graph.

Plain `luxc init` is intentionally offline and dependency-free. Use `--std`
when you want the official standard package set:

```powershell
.\target\release\luxc.exe init ..\my_addon --std
```

Official packages live in
[`TimeWatcher/lux-packages`](https://github.com/TimeWatcher/lux-packages).

Install another official package explicitly:

```powershell
.\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-packages --project ..\my_addon
```

## MGFX

MGFX is the official Lux-adjacent rendering package for Garry's Mod UI:
shader-backed rounded boxes, gradients, rings, arcs, masks, glow, backdrop
effects, image clipping, and text effects while keeping the immediate GLua
drawing model.

Lux projects install it as `@lux/mgfx`. Plain GLua projects can use the
precompiled loader from [`TimeWatcher/lux-mgfx`](https://github.com/TimeWatcher/lux-mgfx);
it installs `_G.MGFX` by default, so existing panels can call
`MGFX.RoundedBox`, `MGFX.TextEx`, and the rest of the facade without adopting
Lux first.

## Editor Tooling

`luxc lsp` is the Lux language server. It is built on the same compiler model
used by builds, so editor behavior follows the Lux version your project
actually uses.

Current editor support includes:

- diagnostics
- hover
- completion
- go to definition
- signature help
- formatting
- semantic tokens
- code actions
- GMod API intelligence
- package source analysis from `lux.lock`

The VS Code extension is intentionally thin: it launches the configured
compiler as `luxc lsp` and handles editor UI. There is no separate LSP binary to
keep in sync with the compiler.

## Quick Start

Lux is currently in alpha. No public binary release is active, so build `luxc`
from source once, then let Lux install its stable user entrypoint:

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release

$Luxc = Resolve-Path .\target\release\luxc.exe
& $Luxc self install --default
$Luxc = Join-Path $env:USERPROFILE ".lux\bin\luxc.exe"
```

This installs:

```text
~/.lux/bin/luxc                         stable entrypoint
~/.lux/toolchains/<version>/luxc        installed compiler version
~/.lux/default-toolchain                selected global default
```

On Windows this is `%USERPROFILE%\.lux\bin\luxc.exe` and
`%USERPROFILE%\.lux\toolchains\<version>\luxc.exe`. Add `~/.lux/bin` to `PATH`
if you want plain `luxc` in every terminal. The VS Code extension also detects
`~/.lux/bin/luxc` directly, so editor support does not require a manual
`lux.compiler.path` setting or PATH setup.

Create an offline, dependency-free project:

```powershell
& $Luxc init ..\my_addon
```

Or create one with `@lux/std` already installed and locked:

```powershell
& $Luxc init ..\my_addon --std
```

Add the official GMod package:

```powershell
& $Luxc install @lux/gmod --from github:TimeWatcher/lux-packages --project ..\my_addon
```

Build the GMod output:

```powershell
& $Luxc gmod build --manifest ..\my_addon\lux.toml
```

For gradual integration without generated GMod loaders, compile a directory of
`.lux` files to matching `.lua` files:

```powershell
& $Luxc build ..\my_addon\src --out ..\my_addon\generated\lua
```

If you clone an example or project that has dependencies but no `lux.lock`, run
the install or lock step before building:

```powershell
& $Luxc lock ..\my_addon
```

Compiler updates are explicit. Once binary releases are published, use:

```powershell
& $Luxc self update
& $Luxc self install 0.1.0-alpha.4 --default
& $Luxc self list
& $Luxc self which
```

Most projects do not need to pin a compiler. Single files and ordinary projects
use the global default. Teams and CI can opt into a project-local pin:

```powershell
& $Luxc self pin 0.1.0-alpha.4 --project .\my_addon
```

## When To Use Lux

Use Lux when you want:

- better GLua-shaped syntax, even in one file
- nil-safe optional data access for player, entity, weapon, UI, config, and hook-time state
- explicit module APIs instead of accidental globals
- checked client/server/shared ownership
- generated loader structure without giving up readable Lua output
- source maps for generated code
- compiler-backed editor diagnostics and navigation
- gradual migration beside existing GLua

Plain GLua may still be enough for tiny throwaway snippets or projects where a
build step is not acceptable.

## Status

Lux is alpha software. The language, package layout, LSP integration, and GMod
backend are usable for experiments and migration work, but breaking changes are
expected while the toolchain stabilizes.

What works today:

- single-file compilation
- modern Lua-shaped syntax
- module directories with multi-part lexical scope
- `client`, `server`, and `shared` declarations and blocks
- explicit `import` / `export` APIs with realm-aware validation
- generated GMod loader trees with optional `autorun` forwarders
- recursive plain Lua directory builds that preserve source-relative paths
- source maps and source comments for generated Lua
- dependency sources from GitHub, URL, or local paths
- `luxc install`, `luxc lock`, `luxc remove`, `luxc doctor`, and `lux.lock`
- `luxc self install`, `luxc self update`, and optional project toolchain pins
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
