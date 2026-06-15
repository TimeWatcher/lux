# GMod Backend and Packaging

This document defines the long-term shape of Lux's GMod backend.

The key rule: GMod is not "just Lua output". It is a deployment target with
realms, file distribution rules, addon layout rules, and packaging artifacts.

## 1. Goals

- compile Lux modules offline
- produce deterministic GLua files
- model client/server/shared realms explicitly
- generate loader files instead of relying on manual include order
- emit source maps/source comments for runtime debugging
- optionally build `.gma` packages through `gmad`

## 2. Realms

Every compiled Lux module belongs to one realm:

- `shared`
- `client`
- `server`

The backend must not infer realm from random string matching in code. Realm
should come from one or more explicit inputs:

- file prefix convention: `cl_`, `sv_`, `sh_`
- nearest source folder convention: `client`, `server`, `shared`
- explicit Lux realm declarations and blocks such as `client fn`,
  `server { ... }`, and `server init { ... }`
- project configuration for external API declarations

Supported source conventions:

```text
src/**/sh_*.lux        -> shared
src/**/cl_*.lux        -> client
src/**/sv_*.lux        -> server
src/**/shared/**/*.lux -> shared
src/**/client/**/*.lux -> client
src/**/server/**/*.lux -> server
```

If a file prefix conflicts with the nearest realm directory, compilation fails.
Without a prefix or realm directory, a part defaults to `shared`.

## 3. Generated Layout

Recommended generated Lua layout:

```text
generated/
  lua/
    lux/
      shared/...
      client/...
      server/...
    autorun/
      lux_<package>_init.lua
      client/lux_<package>_cl_init.lua
      server/lux_<package>_sv_init.lua
```

The exact root can be configured, but generated files should stay separate from
source files.

## 4. Loader Rules

GMod has three different concerns:

- server includes server/shared code
- client includes client/shared code
- server must send client/shared files to clients with `AddCSLuaFile`

The backend should generate loader files from the module graph.

### Shared loader

`lua/autorun/lux_<package>_init.lua` should generally:

- call `AddCSLuaFile` for shared modules
- call `AddCSLuaFile` for client modules
- include shared modules server-side

### Client loader

`lua/autorun/client/lux_<package>_cl_init.lua` should:

- include shared modules client-side
- include client modules client-side

### Server loader

`lua/autorun/server/lux_<package>_sv_init.lua` should:

- include shared modules server-side when not already included by shared loader
- include server modules server-side

The exact duplication-avoidance strategy should be deterministic and tested.

## 5. Import Resolution

Lux imports are resolved offline.

Backend lowering should not generate arbitrary `require` calls for Lux modules.
Instead, the compiler should:

1. build a module graph
2. topologically order modules per realm
3. emit generated Lua chunks
4. emit loader files that include the generated chunks

Runtime `include` should be backend-generated glue, not user-authored Lux import
semantics.

Current implementation status:

- source modules are discovered from `.lux` files under the source root
- a Lux module is a directory module: all part files under the module directory
  share one logical module scope
- `module.lux`, `cl_*.lux`, `sv_*.lux`, `sh_*.lux`, and files under
  `client/server/shared` directories can all be parts of the same module
- multi-part modules must have exactly one entry part named `module.lux` or a
  realm-prefixed `cl_module.lux`, `sv_module.lux`, or `sh_module.lux`
- stable module ids are package-qualified, with generated artifacts qualified
  by realm, for example `my_addon/inventory#client`
- bare imports resolve inside the current package
- imports beginning with `@` resolve external/package ids, for example
  `@lux/std`
- relative imports such as `./button` and `../math` resolve relative to
  the importing module path inside the current package
- named imports are checked against target module exports before code generation
- invalid realm edges are rejected before code generation
- cyclic project module dependencies are rejected
- external runtime imports such as `@lux/std` are tracked as external modules
  instead of requiring a source file
- the current backend injects `lux/std` as a generated runtime module when it
  is imported by project code

Top-level imports are part-local. Top-level declarations are module-private
bindings visible to all parts of the same module, subject to realm checks.
Simple top-level `fn` declarations are hoisted across all parts; top-level
non-function locals are initialized in deterministic part order, and
use-before-initialization is an error.

Part order starts from the entry part, then stable path order, and may be
controlled in source:

```lux
part order { "module", "cl_base", "cl_progress", "cl_rings", "cl_install" }
part before "cl_install"
part after "cl_base"
```

`part order` is for complete arrangement and must live in the entry part.
`part before` and `part after` are auxiliary constraints for small local
adjustments and may live in the affected part. The backend rejects missing
targets, ambiguous short names, duplicate entries in one list, misplaced order
lists, and cycles before resolve/lowering. A metadata-only `module.lux` entry
does not create a generated runtime artifact by itself.

## 6. Exports

Compiled modules may still return an export table internally.

Current backend plan direction:

- each compiled Lux module returns its export table
- generated loader glue includes the module file
- the include result is assigned into a backend-private registry keyed by stable
  module id
- import glue reads from that registry rather than using Lua `require`

The registry is backend implementation detail, not public API. The backend
derives or accepts a per-addon bundle id so multiple Lux addons cannot collide
in the GLua global environment by default.

Current GMod bundles are project-scoped by default:

- every build has a `bundle_id`
- emitted Lua modules live under `lua/lux/<bundle_id>/`
- loader glue stores modules in a bundle-private registry
- logical module ids are only unique inside that project registry
- package/runtime modules must not register themselves in a global table keyed
  by logical id alone

Example loader shape:

```lua
__lux_bundle_my_addon_modules = __lux_bundle_my_addon_modules or {}
local __lux_registry = __lux_bundle_my_addon_modules
local function __lux_import(id)
  local module = __lux_registry[id]
  if module == nil then
    error("Lux module not loaded in bundle my_addon: " .. tostring(id), 2)
  end
  return module
end

do
  if __lux_registry["lux/std"] == nil then
    local __lux_factory = include("lux/my_addon/client/runtime/lux/std.lua")
    __lux_registry["lux/std"] = __lux_factory(__lux_import) or {}
  end
end

do
  if __lux_registry["my_addon/foo#client"] == nil then
    local __lux_factory = include("lux/my_addon/client/my_addon/foo.lua")
    __lux_registry["my_addon/foo#client"] = __lux_factory(__lux_import) or {}
  end
end
```

Generated module files use a factory wrapper so imports stay local to the
loader environment instead of relying on a public global import function:

```lua
return function(__lux_import)
  local __lux_exports = {}
  -- compiled module body
  return __lux_exports
end
```

## 7. Optional GMA Packaging

If `gmad.exe` is available, Lux can produce a `.gma` packaging plan. This is an
optional deployment optimization, not part of the core compiler contract. The
main reason to use it is reducing client Lua download/mount overhead for larger
addons.

Detected local tool path on this machine:

```text
C:\Program Files (x86)\Steam\steamapps\common\GarrysMod\bin\gmad.exe
```

Packaging should be modeled as a build artifact:

```text
GmaPackagePlan
  gmad_path
  addon_json
  output_gma
```

The compiler should generate or validate `addon.json` before invoking `gmad`.

Publishing through `gmpublish.exe` should remain a separate explicit command,
not an accidental side effect of compilation.

Current implementation only builds a command plan equivalent to:

```text
gmad.exe create -folder <addon-root> -out <output.gma>
```

The explicit package command is:

```powershell
cargo run -- gmod package --manifest path\to\lux.toml --gmad path\to\gmad.exe --out dist\addon.gma
```

By default it prints the command after writing generated Lua. It only invokes
`gmad` when `--run` is supplied.

## 8. Source Correlation

Generated GLua should support:

- optional inline `--#lux source: path:line` comments where useful
- sidecar source maps for tool integration
- stable generated file paths for GMod console stack traces

This is required for practical debugging.

Inline source comments are a display/debugging aid, not the only correlation
mechanism. Supported modes are:

- `none`: production-friendly output with sidecar maps only
- `readable`: development default, comments at review anchors such as functions and branch blocks
- `boundary`: comments whenever the mapped source line changes
- `dense`: debug mode, comments for every mapped generated line

Current CLI support:

```powershell
luxc compile examples\smoke.lux --map examples\smoke.lua.map.json
luxc compile examples\smoke.lux --source-comments readable
luxc compile examples\smoke.lux --source-comments dense
luxc map-error examples\smoke.lua.map.json 42
luxc gmod build src path\to\addon
luxc gmod build src path\to\addon --dry-run
luxc gmod build --manifest path\to\lux.toml
luxc gmod api update --out path\to\gmod_api.json --coverage-out path\to\coverage_manifest.json
```

Minimal manifest:

```toml
[gmod]
source_root = "src"
addon_root = "."
generated_root = "generated"
source_comments = "readable"

[target.gmod.realm]
unknown_external = "warn"

[target.gmod.extern]
ThirdPartyAddon = "server"
SharedLibrary = "shared"
net.Start = "server"
```

`package_roots` is optional. When present, it is a comma-separated string of
project-local package roots using the same package directory conventions as
installed package sets. `lux.lock` roots are also loaded automatically, so
project runtime libraries, macros, and host transforms can stay in Lux source
without being embedded in Rust.

`unknown_external` controls globals that are neither Lux bindings nor known
GMod API symbols:

- `allow`: accept silently
- `warn`: accept and report `REALM_UNKNOWN` diagnostics
- `error`: reject, useful for CI or mature projects

The checker uses three categories:

- Lux symbols: strict realm checking
- known GMod API / configured `extern`: strict realm checking
- unknown external: not classified as shared; allowed with warning by default

Source-level extern declarations are also supported:

```lux
extern server ThirdPartyAddon
extern client FancyHud.Open
extern shared SharedLibrary
```

Extern paths use longest-prefix matching, so `extern server net.Start` takes
priority over `extern shared net`.

## 9. Non-Goals

- no in-game Lux compilation
- no runtime parser
- no implicit global module registry exposed as public API
- no automatic Workshop publishing during normal compile
