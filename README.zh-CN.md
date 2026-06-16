<p align="center">
  <img src="images/hero.png" alt="Lux - 面向 Garry's Mod 的 compiler-first 语言和 GLua 工具链" width="100%">
</p>

<h1 align="center">Lux</h1>

<p align="center">
  <strong>一个文件时提供更好的 GLua 语法；项目变大后，由编译器接管 Garry's Mod 工程结构。</strong>
</p>

<p align="center">
  Lux 是面向 Garry's Mod 的开源语言层和工具链。它离线编译成普通、可读的 GLua / Lua 5.1，同时提供 nil-safe 表达式、真实模块、client/server/shared 归属、生成式 GMod loader、source map、无 registry 包系统和编译器驱动的编辑器诊断。
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/zh/">中文文档</a>
  ·
  <a href="#快速开始">快速开始</a>
  ·
  <a href="#一个文件">一个文件</a>
  ·
  <a href="#gmod-项目">GMod 项目</a>
  ·
  <a href="#包系统">包系统</a>
  ·
  <a href="#mgfx">MGFX</a>
  ·
  <a href="README.md">English</a>
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/zh/"><img src="https://img.shields.io/badge/docs-online-2f6feb" alt="Docs"></a>
  <a href="compiler/"><img src="https://img.shields.io/badge/compiler-Rust-f46623" alt="Rust compiler"></a>
  <a href="https://gmod.facepunch.com/"><img src="https://img.shields.io/badge/target-Garry's%20Mod-1f6feb" alt="Garry's Mod target"></a>
  <a href="#授权"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License"></a>
</p>

---

## Lux 是什么

Lux 不是 runtime framework。它不替代 Garry's Mod、GLua 或你已经在用的 API。

它是 compiler-first 的源码层：

```text
Lux source
  -> luxc
  -> readable GLua / Lua 5.1
  -> normal Garry's Mod files
```

Lux 可以以两种规模使用：

```text
one Lux file
  -> 更安全、更有表达力的 GLua 形状语法
  -> 输出普通 Lua

GMod project
  -> module、import、export、realm、package
  -> 生成 loader tree 和 source map
  -> 编译器驱动的 LSP diagnostics
```

输出仍然是可检查的 Lua。已有 GLua、Facepunch API、第三方库、gamemode 和手写入口仍然可以负责运行时行为。

## Lux 解决什么

真实 GMod 代码通常会遇到同一批结构问题。Lux 把这些规则放进语言和编译器，而不是留在项目经验里。

| GLua 项目里的问题 | Lux 的处理方式 |
| --- | --- |
| helper 悄悄变成全局变量 | 模块默认私有，公开 API 必须显式声明 |
| `include` 顺序和 `AddCSLuaFile` 调用变脆 | 编译器生成 loader tree |
| client、server、shared 归属随时间漂移 | `client`、`server`、`shared` 是会被检查的源码声明 |
| player/entity/UI 等可选状态容易造成 nil 崩溃 | `?:` 和 `??` 直接表达可选数据 |
| `condition and a or b` 在 `a` 为 `false` 时出错 | `then ... else ...` 是真正的条件表达式 |
| 生成 Lua 的错误很难追回源码 | source map 和 source comment 保留源码意图 |
| 编辑器只能猜纯文本 | `luxc lsp` 使用编译器 parser、resolver、package graph 和 realm model |

## 值得存在的语法

Lux 保持接近 Lua，但补上 GLua 常见模式真正需要的表达能力。

### 真正的条件表达式

Lua 的伪三元写法在中间值可能为 `false` 时并不安全：

```lua
local enabled = shouldEnable() and false or true
-- enabled becomes true
```

Lux 明确写出分支：

```lux
local enabled = shouldEnable() then false else true
```

### 只对 nil fallback

只有 `nil` 应该 fallback 时使用 `??`。`false` 仍然是有效值。

```lux
local title = panelTitle ?? "Untitled"
local visible = config.visible ?? true
```

### Nil-Safe 访问

可选数据访问保持可见，不需要把每一行都写成嵌套检查。

```lux
local name = player?:Nick() ?? "unknown"
local owner = weapon?:GetOwner()?:Nick() ?? "no owner"
```

这不替代 `IsValid` 检查。它解决的是数据本来就可能缺失时的 nil-indexing 问题。

### Guard 和 Callback

```lux
stopifn valid.is(player)
stopifn data.items

arr.map(players, (player, index) => playerLine(player, index))
```

早退出和小 callback 不再需要比实际逻辑更重的语法噪音。

### Enum 和 Match

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

HUD、武器、实体、UI route、网络消息和 parser 这类状态重的代码，可以把状态名称和状态行为放在一起。

## 写起来是什么样

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

编译器理解 function、guard、enum、match expression、optional access、nil fallback、callback、import、export 和 client/server/shared 归属。

## 一个文件

Lux 可以作为单文件语法升级使用。你不需要 package graph、生成式 addon 布局或 autorun 入口，才能获得语言改进。

```lux
client {
  hook.Add("HUDPaint", "ExampleHud", () => drawHud())
}

server {
  print("server-side setup")
}
```

单文件编译会输出普通 Lua：

```powershell
.\target\release\luxc.exe compile .\hud.lux
```

这个模式适合小脚本、实验、生成片段，或者在已有 GLua 旁边渐进迁移。

## GMod 项目

当源码树变大时，Lux 可以接管那些通常散落在目录约定和手写 loader glue 里的项目结构。

项目模式提供：

- 显式 import 和 export
- 默认私有模块
- 多 part module scope
- `client`、`server`、`shared` 声明
- realm-aware 校验
- 生成 GMod loader tree
- 可选 addon 风格 `autorun` forwarder
- source map 和 source comment
- package 解析
- 编译器驱动的 LSP diagnostics

你不再需要手写维护 loader 顺序：

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

而是在源码里写清归属：

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

然后由 Lux 生成 GMod 侧需要的输出。

## GMod 输出模型

默认项目形态是 addon-oriented：`luxc init` 会写入 `autorun = true`。这意味着 Lux 会生成一个很薄的 `autorun` forwarder，用来 include 生成的 loader。

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

`--no-autorun` 或 `autorun = false` 只禁用这个薄 forwarder。它不代表“gamemode 模式”，也不会禁用生成的 loader tree。已有 gamemode、框架或手写 Lua 入口要自己 include Lux loader 时，才使用这个开关。

两个关键路径是：

- `out`：磁盘上的物理输出根，通常是 `generated/lua`
- `runtime_base`：生成 `include` 和 `AddCSLuaFile` 时使用的 GMod 相对基础路径

这样生成的 include 路径是相对且显式的，不假设所有项目都有同一种目录布局。

最小 manifest：

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

## 包系统

Lux 没有 package registry、镜像源或全局 latest 查询。依赖显式指向：

- GitHub repository
- URL
- 本地 path

GitHub 来源可以用 `tag`、`branch` 或 `commit` 固定，`lux.lock` 记录解析后的 package graph。

普通 `luxc init` 刻意保持离线、无依赖。需要官方标准包时使用 `--std`：

```powershell
.\target\release\luxc.exe init ..\my_addon --std
```

官方包位于
[`TimeWatcher/lux-packages`](https://github.com/TimeWatcher/lux-packages)。

显式安装另一个官方 package：

```powershell
.\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-packages --project ..\my_addon
```

## MGFX

MGFX 是 Lux 生态里的官方 GMod UI 渲染包：shader-backed 圆角框、渐变、
ring、arc、mask、glow、backdrop effect、图片裁剪和文字效果，同时保留
GLua immediate drawing 的使用方式。

Lux 项目可以把它作为 `@lux/mgfx` 安装；纯 GLua 项目也可以直接使用
[`TimeWatcher/lux-mgfx`](https://github.com/TimeWatcher/lux-mgfx) 里的预编译
loader。预编译版本默认安装 `_G.MGFX`，所以现有 panel 可以直接调用
`MGFX.RoundedBox`、`MGFX.TextEx` 等 API，不需要先迁移到 Lux。

## 编辑器工具

`luxc lsp` 是 Lux language server。它建立在和 build 相同的编译器模型上，所以编辑器行为跟随项目实际使用的 Lux 版本。

当前编辑器能力包括：

- diagnostics
- hover
- completion
- go to definition
- signature help
- formatting
- semantic tokens
- code actions
- GMod API intelligence
- 来自 `lux.lock` 的 package source analysis

VS Code 扩展刻意保持很薄：它启动配置的 compiler 执行 `luxc lsp`，并处理编辑器 UI。不存在需要和 compiler 单独同步的 LSP 二进制。

## 快速开始

Lux 当前是 alpha 软件，没有有效的公开二进制 release；请先从源码构建 `luxc`：

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release

$Luxc = Resolve-Path .\target\release\luxc.exe
& $Luxc --help
```

创建离线、无依赖项目：

```powershell
& $Luxc init ..\my_addon
```

或创建已经安装并 lock `@lux/std` 的项目：

```powershell
& $Luxc init ..\my_addon --std
```

添加官方 GMod package：

```powershell
& $Luxc install @lux/gmod --from github:TimeWatcher/lux-packages --project ..\my_addon
```

构建 GMod 输出：

```powershell
& $Luxc gmod build --manifest ..\my_addon\lux.toml
```

如果你 clone 了一个有依赖但没有 `lux.lock` 的 example 或项目，构建前先执行 install 或 lock：

```powershell
& $Luxc lock ..\my_addon
```

## 什么时候适合用 Lux

适合在这些情况下使用 Lux：

- 想要更好的 GLua 形状语法，即使只有一个文件
- player、entity、weapon、UI、config、hook-time state 需要 nil-safe optional access
- 希望用显式 module API 替代意外全局变量
- 需要检查 client/server/shared 归属
- 想生成 loader 结构，但仍保留可读 Lua 输出
- 需要 source map 追踪生成代码
- 需要编译器驱动的编辑器诊断和跳转
- 希望在已有 GLua 旁边渐进迁移

对于一次性小片段，或者不能接受 build step 的项目，普通 GLua 仍然可能足够。

## 状态

Lux 是 alpha 软件。语言、package 布局、LSP 集成和 GMod 后端已经可以用于实验和迁移，但在工具链稳定前仍然会有 breaking changes。

当前可用：

- 单文件编译
- 现代 Lua 形状语法
- 目录模块和多 part 共享词法作用域
- `client`、`server`、`shared` 声明和代码块
- 显式 `import` / `export`，并带 realm-aware 校验
- 生成 GMod loader tree，可选 `autorun` forwarder
- 生成 Lua source map 和 source comments
- 依赖来源支持 GitHub、URL 或本地 path
- `luxc install`、`luxc lock`、`luxc remove`、`luxc doctor` 和 `lux.lock`
- `luxc lsp` 编辑器支持
- compiler checks 和 editor intelligence 共用官方 GMod API 数据

## 文档

- [快速开始](https://timewatcher.github.io/lux-docs-site/zh/guide/getting-started)
- [语言总览](https://timewatcher.github.io/lux-docs-site/zh/language/)
- [模块和 part](https://timewatcher.github.io/lux-docs-site/zh/language/modules)
- [导入和导出](https://timewatcher.github.io/lux-docs-site/zh/language/imports-exports)
- [运行域](https://timewatcher.github.io/lux-docs-site/zh/language/realms)
- [包管理](https://timewatcher.github.io/lux-docs-site/zh/packages/)
- [GMod 后端](https://timewatcher.github.io/lux-docs-site/zh/gmod/)
- [VS Code 和 LSP](https://timewatcher.github.io/lux-docs-site/zh/reference/vscode)
- [标准包](https://github.com/TimeWatcher/lux-packages)
- [LSP 和 VS Code](https://github.com/TimeWatcher/lux-lsp)
- [MGFX](https://github.com/TimeWatcher/lux-mgfx)

## 仓库结构

```text
compiler/        luxc 的 Rust 实现，包括 luxc lsp
lsp/             VS Code 壳和共享 GMod API 智能数据
docs-site/       公开 Lux 文档站，以 submodule 管理
docs/            设计说明和实现参考
examples/        Lux 和 GMod 示例项目
images/          README 和项目媒体资产
```

开发 LSP 或文档站时再初始化对应 submodule：

```powershell
git submodule update --init lsp docs-site
```

## 授权

Lux 使用拆分授权：

- 源码使用 `MIT OR Apache-2.0`，另有独立授权的 package 除外。
- 文档正文使用 `CC-BY-4.0`。
- 文档中的代码示例使用 `MIT OR Apache-2.0`。
- Lux 名称、logo、icon 和其他品牌资产不通过这些开源协议授权复用。

使用 `luxc` 编译你的源码，不会改变你的 addon 或生成项目的授权。如果生成 Lua 嵌入了 Lux runtime 或 package 代码，嵌入的 package 代码保留原授权。

详见 [LICENSE](LICENSE)、[LICENSE-MIT](LICENSE-MIT)、
[LICENSE-APACHE](LICENSE-APACHE)、[LICENSE-DOCS](LICENSE-DOCS) 和
[NOTICE](NOTICE)。
