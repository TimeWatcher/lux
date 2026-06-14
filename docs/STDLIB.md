# Lux Standard Library

## Goals

Lux stdlib is designed for GLua projects, not for replacing all of Lua.

Priorities:

- high performance on LuaJIT/GLua hot paths
- small, explicit modules
- no global monkey-patching
- allocation-aware APIs for per-frame code
- helpers that match common GMod addon patterns

Non-goals:

- class systems
- chain wrapper APIs
- broad compatibility layers
- deep data abstractions
- runtime reflection frameworks

## Package Layout

### `lux/std`

Pure Lua/GLua-safe helpers. This package must not depend on GMod globals.

Exports:

- `arr` for array-like tables
- `dict` for key/value tables
- `set` for boolean set tables
- `str` for string utilities
- `num` for numeric helpers
- `func` for tiny function helpers
- `pool` for table reuse

### `lux/gmod`

GMod-specific helpers. This package may use globals such as `IsValid`, `hook`,
`timer`, `net`, `util`, `player`, `ents`, `Color`, and `vgui`.

Exports:

- `valid`
- `hookx`
- `timerx`
- `netx`
- `json`
- `players`
- `entsx`
- `color`
- `vgui`

## API Conventions

### No Global Patching

The stdlib never mutates `table`, `string`, `math`, or GMod globals.

Use imports:

```lux
import { arr, dict } from "lux/std"
import { hookx, timerx } from "lux/gmod"
```

### Allocation Control

APIs that allocate have simple names:

```lux
local mapped = arr.map(xs, (x) => x + 1)
local keys = dict.keys(tbl)
```

APIs that reuse caller-provided output tables use `Into` or `InPlace`:

```lux
arr.mapInto(xs, callback, out)
arr.filterInto(xs, callback, out)
arr.removeWhereInPlace(xs, callback)
dict.mergeInto(out, defaults, overrides)
players.aliveInto(out)
entsx.byClassInto("prop_physics", out)
```

This is the preferred style for hooks, HUD painting, VGUI layout, Think loops,
and other frequently executed GMod paths.

### Nil Semantics

Lux language defaults and `??` are nil-only. The stdlib follows the same rule
where fallback behavior exists. `false` is a real value, not missing data.

Table spread and `dict.mergeInto` ignore `nil` spread/source tables to make
props/default merging cheap and convenient.

### Arrays vs Dictionaries

`arr` assumes a dense one-based Lua array and uses `#input`.

`dict` assumes key/value tables and uses `pairs`.

Do not use `arr` helpers for sparse dictionaries.

## `lux/std` Modules

### `arr`

Core operations:

- `clear(input)`
- `copy(input)`
- `copyInto(input, out)`
- `map(input, callback)`
- `mapInto(input, callback, out)`
- `filter(input, callback)`
- `filterInto(input, callback, out)`
- `reduce(input, initial, callback)`
- `some(input, callback)`
- `every(input, callback)`
- `find(input, callback)`
- `indexOf(input, needle)`
- `contains(input, needle)`
- `removeValue(input, needle)`
- `removeWhereInPlace(input, callback)`
- `compactInPlace(input)`
- `pushAllInto(out, input)`

Callback convention:

```lux
(value, index) => ...
```

### `dict`

Core operations:

- `clear(input)`
- `count(input)`
- `isEmpty(input)`
- `keys(input)`
- `keysInto(input, out)`
- `values(input)`
- `valuesInto(input, out)`
- `copy(input)`
- `copyInto(input, out)`
- `merge(...)`
- `mergeInto(out, ...)`
- `defaultsInto(out, defaults)`
- `pick(input, keys)`
- `pickInto(input, keys, out)`
- `omit(input, omitted)`
- `omitInto(input, omitted, out)`
- `shallowEqual(left, right)`

`mergeInto(out, ...)` copies sources from left to right. Later values override
earlier values. `nil` sources are skipped.

### `set`

Boolean-table set helpers:

- `fromArray(input)`
- `add(input, value)`
- `remove(input, value)`
- `has(input, value)`
- `unionInto(out, left, right)`
- `intersectInto(out, left, right)`
- `differenceInto(out, left, right)`

### `str`

Common string helpers:

- `startsWith(value, prefix)`
- `endsWith(value, suffix)`
- `contains(value, needle)`
- `trim(value)`
- `escapePattern(value)`
- `split(value, separator)`
- `splitInto(value, separator, out)`
- `join(input, separator)`
- `truncate(value, maxLength, suffix = "...")`
- `formatBytes(bytes)`

`str.contains` and `str.splitInto` use plain substring search where applicable,
not Lua pattern matching.

### `num`

Numeric helpers:

- `clamp(value, minValue, maxValue)`
- `lerp(t, from, to)`
- `round(value, step = 1)`
- `sign(value)`
- `approach(current, target, delta)`
- `remap(value, inMin, inMax, outMin, outMax)`
- `nearlyEqual(left, right, epsilon = 0.000001)`

### `func`

Tiny function helpers:

- `noop()`
- `identity(value)`
- `once(callback)`
- `try(callback, fallback)`

`func.try` is intentionally small. It wraps a zero-argument callback with
`pcall` and returns `fallback` on failure.

### `pool`

Table reuse helpers:

- `new()`
- `acquire(bucket)`
- `release(bucket, item, clear = true)`
- `clearTable(input)`

`pool.release` clears table fields by default before putting the table back.

## `lux/gmod` Modules

### `valid`

Helpers around `IsValid`:

- `is(value)`
- `orNil(value)`
- `orFallback(value, fallback)`
- `all(values)`
- `any(values)`
- `filterInto(values, out)`

### `hookx`

Lifecycle-aware hook helpers:

- `handle(event, id)`
- `add(event, id, callback)`
- `remove(handleOrEvent, id)`
- `replace(event, id, callback)`
- `once(event, id, callback)`
- `object(event, object, callback)`
- `scoped(owner, event, callback)`

Most registration helpers return `{ event, id }` so callers can remove hooks
without reconstructing identifiers.

### `timerx`

Timer helpers:

- `id(prefix = "lux.timer")`
- `handle(id)`
- `after(delay, callback, id)`
- `every(interval, callback, repetitions = 0, id)`
- `cancel(handleOrId)`
- `exists(handleOrId)`
- `simple(delay, callback)`

Generated timer ids use a module-local counter. They do not allocate temporary
tables.

### `netx`

Thin `net` helpers:

- `register(name)`
- `receive(name, callback)`
- `start(name)`
- `broadcast(name, writer)`
- `sendTo(name, target, writer)`
- `sendOmit(name, omitted, writer)`
- `withMessage(name, writer)`

`register` uses a `server { ... }` realm block internally, so
`util.AddNetworkString` is emitted only into the server artifact. Prefer Lux
realm syntax (`server { ... }`, `client { ... }`, and realm-marked
declarations) over runtime guard helpers.

### `json`

Thin wrappers over `util.TableToJSON` and `util.JSONToTable`:

- `encode(value, pretty = false)`
- `decode(value)`
- `decodeOrNil(value)`

### `players`

Player helpers:

- `all()`
- `findBySteamID64(steamId64)`
- `aliveInto(out)`
- `humansInto(out)`

### `entsx`

Entity query helpers:

- `byClassInto(className, out)`
- `nearbyInto(origin, radius, out, className)`

### `color`

Color helpers:

- `rgb(r, g, b)`
- `rgba(r, g, b, a)`
- `byte(value)`
- `copy(input)`
- `withAlpha(input, alpha)`
- `lerp(t, from, to)`
- `toHex(input)`

`color.lerp` clamps interpolated channels to byte range before constructing the
GMod `Color`.

### `vgui`

Small VGUI safety helpers:

- `safeRemove(panel)`
- `clearChildren(panel)`
- `setTextIfChanged(panel, text)`

These helpers do not try to abstract VGUI. They only remove repetitive safety
checks and avoid unnecessary setter calls.
