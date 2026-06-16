# Lux Examples

This directory keeps source examples and generated Lua snapshots side by side.

## Feature Tour

`features.lux` is a single-file tour of currently implemented language features:

- import/export
- namespace imports and `as` aliases
- `const` exports and immutable local bindings
- compile-time macro imports
- expression macros in nested value positions
- lexical `fn` with implicit return
- `then/else` conditional expressions
- nil coalescing with `??`
- safe field/index/method access with `?.`, `tbl?.[key]`, and `?:`
- template strings
- compound assignment
- destructuring
- table spread
- pipeline placeholder calls
- do expressions
- normal arrow functions `=>`
- implicit-self callbacks `->`
- tail table calls
- GLua method declarations such as `fn PANEL:Paint`
- GLua multi-return preservation in tail-sensitive positions
- `lux/std`, `lux/reactive`, and `lux/gmod` runtime imports
- host transforms for `lux/ui` tail-table call chains
- scalar enums with zero-runtime `repr number` and `repr string`
- `repr existing` enum views over existing table layouts
- `match` expressions in return/local value contexts
- or-patterns such as `A | B | C`
- `stopif`/`stopifn`, `breakif`, and `continueif`
- match codegen that skips proven-unreachable arms

Regenerate the Lua snapshot:

```powershell
cd C:\development\gmod\lux\compiler
$lua = cargo run -- compile ..\examples\features.lux --map ..\examples\features.lua.map.json --source-comments readable
[System.IO.File]::WriteAllText((Resolve-Path ..\examples\features.lua), (($lua -join "`r`n") + "`r`n"), [System.Text.UTF8Encoding]::new($false))
cargo run -- lint ..\examples\features.lux
cargo run -- format ..\examples\features.lux --check
cargo run -- map-error ..\examples\features.lua.map.json 210
```

## GMod Project

`gmod_project` demonstrates the project/module system:

- `lux.toml` manifest
- shared/client/server realms
- cross-module import/export validation
- `lux/std`, `lux/gmod`, `lux/reactive`, and `lux/ui` runtime package injection from package manifests
- `lux/ui` host transform folding from compile-time Lux code into runtime `node` calls
- GMod macros such as `defineHook` and `defineNetReceiver`
- enum + match use inside shared gameplay/HUD code
- early-return and loop-control shortcuts in project code
- generated loader files and wrapped modules
- readable inline `--#lux source:` comments plus sidecar `.lua.map.json` files

Regenerate the GMod output snapshot:

```powershell
cd C:\development\gmod\lux\compiler
Push-Location ..\examples\gmod_project
cargo run --manifest-path ..\..\compiler\Cargo.toml -- install @lux/std --from github:TimeWatcher/lux-packages
cargo run --manifest-path ..\..\compiler\Cargo.toml -- install @lux/gmod --from github:TimeWatcher/lux-packages
Pop-Location
cargo run -- gmod build --manifest ..\examples\gmod_project\lux.toml
```

Optional `.gma` packaging is explicit. It is useful when you want to reduce
client Lua download/mount overhead, but normal development does not require it:

```powershell
cargo run -- gmod package --manifest ..\examples\gmod_project\lux.toml --gmad "C:\Program Files (x86)\Steam\steamapps\common\GarrysMod\bin\gmad.exe" --out ..\examples\gmod_project\dist\lux.gma
```

The generated output is written to:

```text
examples/gmod_project/generated/
```

## Diagnostic Example

`match_diagnostics.lux` intentionally contains a non-exhaustive enum match and
unreachable match arms. It is useful for checking `MATCH001` and `MATCH002`:

```powershell
cd C:\development\gmod\lux\compiler
cargo run -- compile ..\examples\match_diagnostics.lux
```

`invalid_project` intentionally imports a missing runtime export. It is useful
for checking module diagnostics:

```powershell
cd C:\development\gmod\lux\compiler
cargo run -- gmod build --manifest ..\examples\invalid_project\lux.toml --dry-run
```
