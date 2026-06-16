<p align="center">
  <img src="images/hero.png" alt="Lux - 面向 Garry's Mod addon 开发的现代语言和 GLua 工具链" width="100%">
</p>

<h1 align="center">Lux</h1>

<p align="center">
  <strong>面向 Garry's Mod addon 开发的现代语言和 compiler-first 工具链。</strong>
</p>

<p align="center">
  用更有表达力的 Lux 写源码，编译成可读 GLua，把模块、realm、loader、source map、diagnostics、package 和编辑器智能交给编译器处理。
</p>

<p align="center">
  <a href="https://timewatcher.github.io/lux-docs-site/zh/">中文文档</a>
  ·
  <a href="#快速开始">快速开始</a>
  ·
  <a href="#语法预览">语法预览</a>
  ·
  <a href="#gmod-工具链">GMod 工具链</a>
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

## 为什么用 Lux?

GLua 很强，但真实 Garry's Mod 项目很快会遇到同一批结构问题：全局变量泄漏成意外
API，`include` 顺序变成经验规则，客户端/服务端/shared 边界逐渐漂移，生成 Lua 的
报错很难定位回源意图，编辑器也只能靠纯文本猜测。

Lux 保留 Lua / GLua 的手感，但把项目结构交给编译器。

你写 Lux。Lux 输出普通、可检查的 GLua。

| GLua 项目里的痛点 | Lux 的处理方式 |
| --- | --- |
| helper 泄漏成全局变量 | 模块默认私有，公开 API 必须显式 `export` |
| `AddCSLuaFile` 和 `include` 顺序变成项目传说 | realm 是语言模型的一部分，GMod loader 由编译器生成 |
| client/server/shared 代码容易混错 | `client`、`server`、`shared` 声明会被检查 |
| 大 addon 需要更强表达力 | `fn`、guard、enum、`match`、可选访问、`??`、箭头函数、导入导出 |
| 生成代码或运行时报错难追踪 | source map 把输出位置映射回 Lux 源码 |
| 编辑器只能猜 Lua 文本 | `luxc lsp` 使用和构建相同的 parser、resolver、package graph 和 realm checker |

## 语法预览

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

Lux 仍然接近 Lua，但编译器理解更多 addon 结构：

- `fn` 声明，支持 block 或 expression body
- `stopif` / `stopifn` 风格的 guard exit
- `enum` 和 `match` 描述显式状态
- `?:` 可选访问和 `??` nil fallback
- 箭头函数用于 callback
- 显式 import / export
- `client fn` 这样的 realm-aware 声明

## GMod 工具链

Lux 不是 runtime framework。它不替代 Garry's Mod、GLua 或你已经在用的 API。

它是离线 compiler 和项目工具链。

```text
Lux source
   |
   v
luxc gmod build
   |
   +- 解析模块和 package
   +- 检查 client/server/shared realm
   +- 生成 GMod loader tree
   +- 输出可读 GLua
   +- 写出 source map
   |
   v
generated/lua/
   +- autorun/          可选 addon forwarder
   +- lux/<bundle>/     生成 loader 和 module artifact
   +- *.lua.map.json    source map
```

输出是普通 GLua/Lua 5.1，可以检查、调试并随 addon 发布。如果已有 gamemode、框架或
手写 Lua 入口负责启动，设置 `autorun = false` 或传 `--no-autorun`；Lux 仍然会生成
loader tree。

## 核心能力

### 现代语法，Lua 形状

- `fn` function
- block 和 expression body
- guard statement
- arrow callback
- optional access
- nil coalescing
- template string
- destructuring
- table spread
- pipeline
- enum
- checked `match`

### 显式模块

Lux 模块默认私有。公开 API 要主动声明。

```lux
fn normalizeHealth(hp) =
  hp < 0 then 0 else hp > 100 then 100 else hp

export { normalizeHealth }
```

### Realm-Aware 代码

client、server 和 shared 归属是源码模型的一部分。

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

Lux 能推断声明属于哪个运行域，并生成 Garry's Mod 需要的 loader 结构。

### 编译器驱动的编辑器支持

`luxc lsp` 使用和构建相同的编译器模型提供编辑器能力：

- diagnostics
- hover
- completion
- go to definition
- signature help
- formatting
- semantic tokens
- code actions
- GMod API documentation

VS Code 扩展刻意保持很薄：它只启动选中的 compiler 执行 `luxc lsp`，让编辑器行为和
项目实际构建使用的 Lux 版本保持一致。

### 无 Registry 包系统

Lux 没有 package registry、镜像源或全局 latest 查询。依赖显式指向 GitHub、URL 或本地
path。GitHub 来源可以用 `tag`、`branch` 或 `commit` 固定，`lux.lock` 记录解析后的
package graph。

官方标准包位于
[`TimeWatcher/lux-packages`](https://github.com/TimeWatcher/lux-packages)。

## 快速开始

Lux 当前是 alpha 软件，没有有效的公开二进制 release；请先从源码构建 `luxc`。

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release
.\target\release\luxc.exe --help
```

创建不访问网络的项目：

```powershell
.\target\release\luxc.exe init ..\my_addon
```

创建带标准包配置的项目：

```powershell
.\target\release\luxc.exe init ..\my_addon --std
```

安装官方 GMod package：

```powershell
Push-Location ..\my_addon
..\lux\compiler\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-packages
Pop-Location
```

构建 Garry's Mod 输出：

```powershell
.\target\release\luxc.exe gmod build --manifest ..\my_addon\lux.toml
```

最小 manifest：

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

`out` 是物理输出根。`runtime_base` 是生成 `include` 和 `AddCSLuaFile` 时使用的 GMod
相对路径。`autorun` 只控制 `out/autorun` 下的 addon 风格 forwarder。

## 什么时候适合用 Lux

Lux 适合：

- 新 Garry's Mod addon
- client/server/shared 结构逐渐变复杂的 gamemode
- 需要私有模块和显式公开 API 的 addon
- 想要更好编辑器诊断的项目
- 在已有 GLua 旁边渐进迁移
- loader 顺序已经难以维护的代码库

Lux 未必适合：

- 很小的单文件脚本
- 一次性测试片段
- 普通 GLua 已经足够的 addon

## 状态

Lux 是 alpha 软件。语言、package 布局、LSP 集成和 GMod 后端已经可以用于实验和迁移，
但在工具链稳定前仍然会有 breaking changes。

当前可用：

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

使用 `luxc` 编译你的源码，不会改变你的 addon 或生成项目的授权。如果生成 Lua 嵌入了
Lux runtime 或 package 代码，嵌入的 package 代码保留原授权。

详见 [LICENSE](LICENSE)、[LICENSE-MIT](LICENSE-MIT)、
[LICENSE-APACHE](LICENSE-APACHE)、[LICENSE-DOCS](LICENSE-DOCS) 和
[NOTICE](NOTICE)。
