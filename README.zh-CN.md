<div align="center">

# Lux

**面向 Garry's Mod addon 的编译型语言和工具链，目标是摆脱手写 loader、全局表和脆弱的
realm 顺序。**

[![Release](https://img.shields.io/github/v/release/TimeWatcher/lux?label=release)](https://github.com/TimeWatcher/lux/releases)
[![Docs](https://img.shields.io/badge/docs-online-2f6feb)](https://timewatcher.github.io/lux-docs-site/zh/)
[![Rust](https://img.shields.io/badge/compiler-Rust-f46623)](compiler/)
[![Garry's Mod](https://img.shields.io/badge/target-Garry's%20Mod-1f6feb)](https://gmod.facepunch.com/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#授权)

[中文文档](https://timewatcher.github.io/lux-docs-site/zh/) ·
[快速开始](#快速开始) ·
[内置包](https://github.com/TimeWatcher/lux-packages) ·
[LSP](https://github.com/TimeWatcher/lux-lsp) ·
[MGFX](https://timewatcher.github.io/mgfx-docs-site/zh/) ·
[发布下载](https://github.com/TimeWatcher/lux/releases)

[English](README.md) · 简体中文

</div>

Lux 是一门贴近 Lua 的 Garry's Mod 开发语言。它离线编译到普通 GLua/Lua 5.1，生成结果
仍然可读，同时把真实项目里最容易出错的部分交给编译器处理：模块边界、运行域加载、
包发现、显式导出、source map 和诊断。

Lux 不是接管 addon 的运行时框架。它是一个编译器：你写更清晰的 GLua 项目，它输出
Garry's Mod 能直接加载的 Lua 和 loader。

## 为什么需要 Lux

GMod addon 通常会同时遇到这些问题：

| GLua 项目里的问题 | Lux 的处理方式 |
| --- | --- |
| 私有 helper 很容易变成意外公开 API。 | 目录模块默认私有，只暴露显式导出的名字。 |
| realm 加载充满 `AddCSLuaFile`、`include` 顺序和文件名约定。 | `client`、`server`、`shared`、realm block 和生成 loader 是语言模型的一部分。 |
| 大型 addon 需要结构，但不能牺牲 GLua 生态兼容。 | Lux 编译到可读 Lua，并允许正常调用 GMod API 和第三方库。 |

## 代码观感

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

它仍然接近 Lua，但有模块私有声明、明确的运行域、enum 和 `match`、表达式返回、箭头
函数、可选访问、nil 合并，以及能描述真实公开 API 的导出语义。

## 核心特性

- **目录就是模块**：一个模块是多个 part 文件共享的逻辑 module scope，不需要给每个
  package 写繁琐 manifest。
- **公开 API 显式声明**：`export { public_name = local_binding }` 把内部名字映射到
  外部 API 名字，未导出的内容保持私有。
- **运行域是一等概念**：`client fn`、`server fn`、`shared` 代码，以及
  `client { ... }` / `server { ... }` block 直接表达 GMod 执行环境。
- **GMod loader 由编译器生成**：批量处理客户端文件，避免 addon 全局 `lua/` 目录重名，
  并保留 source map 和调试信息。
- **更有表达力的语法**：`match`、`then/else`、箭头函数、可选调用、解构、table spread、
  pipeline helper 和隐式表达式返回。
- **按目录约定发现包**：运行时、编译期、macro 和 host 代码按目录布局发现，而不是靠
  手写 package manifest。

## 快速开始

从 [Releases](https://github.com/TimeWatcher/lux/releases) 下载最新 Windows 构建，解压
后保持 `packages` 目录和 `luxc.exe` 放在一起：

```text
luxc-v0.1.0-x86_64-pc-windows-msvc/
  luxc.exe
  packages/
```

运行：

```powershell
.\luxc.exe --help
.\luxc.exe compile .\src\module.lux
```

构建一个 GMod addon 项目：

```powershell
.\luxc.exe gmod build --manifest .\lux.toml
```

一个最小 GMod manifest：

```toml
[gmod]
source_root = "src"
addon_root = "generated"
package_id = "my_addon"
bundle_id = "my_addon"

[target.gmod.realm]
unknown_external = "warn"
```

## 从源码构建

```powershell
git clone --recurse-submodules https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo test
cargo build --release
```

编译器会生成在：

```text
compiler/target/release/luxc.exe
```

常用开发命令：

```powershell
cargo run -- compile ..\examples\features.lux
cargo run -- gmod build --manifest ..\examples\gmod_project\lux.toml --dry-run
```

## 仓库结构

```text
compiler/        luxc 的 Rust 实现
packages/        内置 Lux packages，以 submodule 管理
lsp/             Lux LSP、VS Code 支持和 GMod API 智能标准
docs-site/       Lux 文档站源码，以 submodule 管理
mgfx-docs-site/  MGFX 文档站源码，以 submodule 管理
docs/            设计说明和实现参考
examples/        Lux 和 GMod 示例项目
```

如果 clone 时没有拉取 submodule：

```powershell
git submodule update --init --recursive
```

## CLI 概览

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

## 文档

- [快速开始](https://timewatcher.github.io/lux-docs-site/zh/guide/getting-started)
- [语言总览](https://timewatcher.github.io/lux-docs-site/zh/language/)
- [模块和 part](https://timewatcher.github.io/lux-docs-site/zh/language/modules)
- [导入和导出](https://timewatcher.github.io/lux-docs-site/zh/language/imports-exports)
- [运行域](https://timewatcher.github.io/lux-docs-site/zh/language/realms)
- [GMod 后端](https://timewatcher.github.io/lux-docs-site/zh/gmod/)
- [LSP 和 VS Code 标准](https://github.com/TimeWatcher/lux-lsp)
- [MGFX package 文档](https://timewatcher.github.io/mgfx-docs-site/zh/)

## 状态

Lux 当前是早期 `0.1.0` 编译器版本。语言、package 布局和 GMod 后端已经可以用于实验和
迁移，但仍应按 pre-1.0 项目看待。

## 参与开发

编译器：

```powershell
cd compiler
cargo test
```

文档：

```powershell
cd docs-site
npm install
npm run dev -- --host 127.0.0.1 --port 4173
npm run build
```

MGFX 文档：

```powershell
cd mgfx-docs-site
npm install
npm run dev -- --host 127.0.0.1 --port 4174
npm run build
```

Package 位于 `packages` submodule。修改 package 时应直接进入该仓库，并通过编译器测试
或导入该 package 的 GMod 项目构建验证。

语言服务和 VS Code 支持标准位于 `lsp` submodule。开发编辑器集成、GMod API 智能、
hover、completion、diagnostics 或 quick fix 时，应直接进入该仓库修改。

## 授权

Lux 使用拆分授权：

- 源码使用 `MIT OR Apache-2.0`，另有独立授权的 package 除外。
- 内置 `@lux/mgfx` package 仅允许非商业使用；商业使用 MGFX 需要版权方单独书面授权。
- 文档正文使用 `CC-BY-4.0`。
- 文档中的代码示例使用 `MIT OR Apache-2.0`。
- Lux 名称、logo、icon 和其他品牌资产不通过这些开源协议授权复用。

使用 `luxc` 编译你的源码，不会改变你的 addon 或生成项目的授权。如果生成 Lua 嵌入了
Lux runtime 或 package 代码，嵌入的 package 代码保留原授权：大部分 Lux 代码仍是
`MIT OR Apache-2.0`，MGFX 代码仍受 Lux MGFX Non-Commercial License 约束。

详见 [LICENSE](LICENSE)、[LICENSE-MIT](LICENSE-MIT)、
[LICENSE-APACHE](LICENSE-APACHE)、[LICENSE-DOCS](LICENSE-DOCS) 和
[NOTICE](NOTICE)。MGFX 见 `packages/LICENSE-MGFX-NC`。
