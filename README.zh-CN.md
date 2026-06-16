<div align="center">

# Lux

**面向 Garry's Mod addon 的编译器优先语言：给项目结构、模块和 realm 检查，但不牺牲
可读 GLua 输出。**

[![Docs](https://img.shields.io/badge/docs-online-2f6feb)](https://timewatcher.github.io/lux-docs-site/zh/)
[![Rust](https://img.shields.io/badge/compiler-Rust-f46623)](compiler/)
[![Garry's Mod](https://img.shields.io/badge/target-Garry's%20Mod-1f6feb)](https://gmod.facepunch.com/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#授权)

[中文文档](https://timewatcher.github.io/lux-docs-site/zh/) ·
[快速开始](#快速开始) ·
[标准包](https://github.com/TimeWatcher/lux-std) ·
[VS Code](https://github.com/TimeWatcher/lux-lsp) ·
[MGFX](https://github.com/TimeWatcher/lux-mgfx)

[English](README.md) · 简体中文

</div>

Lux 是面向 Lua / GLua 的 Garry's Mod 开发语言和工具链。它在游戏外离线编译到普通
GLua/Lua 5.1，生成结果保持可读，并把真实 addon 开发里最容易出错的部分交给编译器：
模块、realm 加载、导入导出、source map、包解析和编辑器诊断。

Lux 不是接管 addon 的运行时框架。它是一个编译器，可以用于新 addon、gamemode，也
可以逐步接入已有 GLua 项目。

## 为什么用 Lux

| GLua 项目里的痛点 | Lux 的处理方式 |
| --- | --- |
| 私有 helper 很容易漏成全局变量。 | 目录模块默认私有，只暴露显式 `export` 的名字。 |
| `AddCSLuaFile` 和 `include` 顺序变成项目传说。 | realm 是语言模型的一部分，GMod loader 从模块图生成。 |
| 大 addon 需要结构，但仍然离不开 GMod API。 | Lux 输出可读 Lua，并允许普通 GMod / 第三方调用穿透。 |
| 编辑器只能靠文本猜测。 | `luxc lsp` 使用和构建相同的 parser、resolver、包图和 realm checker。 |
| 标准库不应该跟编译器 release 绑定。 | 官方包在独立 `lux-std` 仓库，项目用 `lux.lock` 固定。 |

## 当前可用

- 目录模块和多 part 共享词法作用域
- `client`、`server`、`shared` 声明和代码块
- 显式 `import` / `export`，并带 realm-aware 校验
- 生成 GMod loader tree，可选 addon 风格 `autorun` forwarder
- 生成 Lua source map 和 source comments
- 无 registry 包模型：依赖显式指向 GitHub、URL 或本地 path
- `luxc install`、`luxc lock`、`luxc remove`、`luxc doctor` 和 `lux.lock`
- `luxc lsp` 支持 VS Code hover、completion、definition、signature help、diagnostics、formatting 和 GMod API 文档
- compiler 和 editor 共用官方 GMod API 数据库

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

语法仍然接近 Lua，但模块默认私有，公开 API 必须显式声明，realm 所属会被检查，GMod
里常见的 nil 调用更容易写，编辑器看到的是同一套编译器语义。

## 快速开始

当前没有有效的公开二进制 release。alpha 阶段请从源码构建 `luxc`：

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo build --release
.\target\release\luxc.exe --help
```

初始化项目默认离线执行：

```powershell
.\target\release\luxc.exe init .\my_addon
```

只有需要官方包时才显式安装：

```powershell
.\target\release\luxc.exe init .\my_addon --std
.\target\release\luxc.exe install @lux/gmod --from github:TimeWatcher/lux-std --project .\my_addon
```

Lux 没有 package registry。依赖来源和版本由 `lux.toml` 里的显式 `github`、`url` 或
`path` 条目决定，并可用 `tag`、`branch` 或 `commit` 固定。`lux.lock` 记录已解析的
package set。`luxc lock` 只按 manifest 重新生成 lockfile，不查找新版本；`luxc remove`
删除直接依赖并剪掉不再使用的传递包。

构建 GMod 项目：

```powershell
.\target\release\luxc.exe gmod build --manifest .\lux.toml
```

当已有 gamemode、框架或手写 Lua 入口负责启动时，使用 `--no-autorun` 或
`autorun = false`。这只关闭 `out/autorun` 下的薄 forwarder，Lux loader tree 仍然会生成。

## 最小 Manifest

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
相对路径。`autorun` 只控制 `out/autorun` 下的 addon 风格 forwarder，不会关闭 Lux
loader tree。

## 从源码构建

```powershell
git clone https://github.com/TimeWatcher/lux.git
cd lux\compiler
cargo test
cargo build --release
```

编译器二进制输出到：

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
compiler/        luxc 的 Rust 实现，包括 luxc lsp
lsp/             VS Code 壳和共享 GMod API 智能数据
docs-site/       公开 Lux 文档站，以 submodule 管理
docs/            设计说明和实现参考
examples/        Lux 和 GMod 示例项目
```

开发 LSP 或文档站时再初始化对应 submodule：

```powershell
git submodule update --init lsp docs-site
```

## CLI 概览

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

## 文档

- [快速开始](https://timewatcher.github.io/lux-docs-site/zh/guide/getting-started)
- [语言总览](https://timewatcher.github.io/lux-docs-site/zh/language/)
- [模块和 part](https://timewatcher.github.io/lux-docs-site/zh/language/modules)
- [导入和导出](https://timewatcher.github.io/lux-docs-site/zh/language/imports-exports)
- [运行域](https://timewatcher.github.io/lux-docs-site/zh/language/realms)
- [包管理](https://timewatcher.github.io/lux-docs-site/zh/packages/)
- [GMod 后端](https://timewatcher.github.io/lux-docs-site/zh/gmod/)
- [VS Code 和 LSP](https://timewatcher.github.io/lux-docs-site/zh/reference/vscode)
- [LSP 仓库](https://github.com/TimeWatcher/lux-lsp)
- [MGFX 仓库](https://github.com/TimeWatcher/lux-mgfx)

## 状态

Lux 当前是 alpha 软件，没有有效的公开二进制 release。语言、package 布局、LSP 集成和
GMod 后端已经可以用于实验和迁移，但仍应按 pre-1.0 项目看待。

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

官方标准 package 位于独立的 [`lux-std`](https://github.com/TimeWatcher/lux-std)
仓库。修改 package 时应直接进入该仓库，并通过编译器测试或导入该 package 的 GMod
项目构建验证。

VS Code 支持和 GMod API 智能数据位于 `lsp` submodule。语言服务本身由 `luxc lsp`
提供；修改 hover、completion、signature help、diagnostics 或 quick fix 时，应修改
compiler。LSP 使用 compiler 的 package resolution 和 module analysis，因此跨 part 和
imported definition 会与当前选中的 `luxc` 保持一致。

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
