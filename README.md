# Notist

**Note + -ist → notist（also not an -ist）**

Notist 是一门带静态类型系统的文档编程语言，为取代 Markdown 而生，文件扩展名为 `.not`。它针对 Markdown 的几个根本问题：

- 没有"官方"实现，方言即分裂
- 内容表达能力弱，扩展靠方言，方言语法混乱
- 引用受路径影响且粒度粗
- 缺少 Agent 原生设计

它在语法与设计上有很多地方受 Rust 和 Typst 启发：Markup/Code 双模式、静态类型与诊断、模块路径、`@[]` 属性标注等等。

```not
= 部署手册

#callout(kind: "warning", title: [先验证])[
  修改后运行 `notist check`，语义见 #<vault::cli::inspect>。
]

- 条目支持 *强调*、_斜体_ 与行内 `code`
```

## 仓库结构

- `crates/` — Rust 工作区：`notist-syntax`（文法）、`notist-analysis`（分析）、`notist-service`（服务核心）、`notist-cli`（CLI 与 LSP）、`notist-plugin-host` / `notist-plugin-sdk`（插件系统）、`notist-html`（站点产出）、`notist-eval` / `notist-model`
- `plugins/` — 官方插件（core、mermaid、shader 等）
- `docs/` — 用 Notist 自举维护的文档 Vault（`Notist.toml` 为根），**文档一律 `.not`，不使用 `.md`**
- `skills/` — 面向 Agent 的 notist 使用技能（也可由 `notist skill` 生成）

## 安装

从源码安装（需要较新的 stable Rust，edition 2024）：

```sh
cargo install --path crates/notist-cli
```

或通过 Nix flake：

```sh
nix profile install github:AzurIce/notist
```

项目尚未发布到 crates.io，也没有预编译产物。CLI 二进制在构建时内嵌官方文档 Vault 与 Agent Skill，安装即自带完整教学资源。

## 基本使用

在 Vault 根（含 `Notist.toml` 的目录）下：

```sh
notist inspect status --vault docs         # Vault、快照、诊断与索引概览
notist check --vault docs                   # 校验模块与引用
notist inspect search "workspace snapshot" --vault docs   # 检索（支持 --exact / --fuzzy）
notist inspect read docs/grammar.not --vault docs    # 读取源文并标注生效属性环境
notist inspect references vault::intro --vault docs   # 逻辑模块的引用
```

命令面没有写命令：用任何编辑器修改 `.not` 文件，daemon 通过 watcher 感知变更，改完跑 `notist check` 验证。`notist build` 与 `notist preview` 负责站点产出，`notist lsp` 供编辑器接入（Zed / Obsidian 插件即基于它）。完整命令规范见 `docs/cli/README.not`。

## Agent 接入

`notist skill init <dir>` 从二进制内嵌资源生成官方 Agent Skill（单文件 `SKILL.md`，与本仓库 `skills/notist` 同源），放进你的 Agent 技能目录即可：

```sh
notist skill init .agents/skills/notist
```

Skill 覆盖查询与导航命令、Selector/Scope 寻址、完整结果契约（无翻页、无截断）等约定，Agent 无需阅读源码或文档 Vault 就能正确驱动 CLI。

## 文档

`docs/README.not` 是文档 Vault 的入口：语法参考、内置构造器、类型系统、CLI 规范、设计记录都从那里引用。仓库访客可直接读文件，Agent 与本机用户建议通过 `notist` CLI 查询（`notist inspect status` / `inspect search` / `inspect read` 等，见 `docs/cli/README.not`）。

## 相关项目

- [zed-notist](https://github.com/AzurIce/zed-notist) — Zed 编辑器扩展
- [obsidian-notist](https://github.com/AzurIce/obsidian-notist) — Obsidian 插件（独立 Notist World + tree-sitter 高亮 + LSP 客户端）
- [tree-sitter-notist](https://github.com/AzurIce/tree-sitter-notist) — tree-sitter 语法的上游

## 状态

项目处于活跃开发中，从未发布过任何版本；命令面与语法仍会以破坏性方式调整。
