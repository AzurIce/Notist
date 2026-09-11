# Notist

*Note + -ist → notist（also not an -ist）*

Notist 是一门带静态类型系统的文档编程语言，为取代 Markdown 而生，文件扩展名为 `.not`。它针对 Markdown 的几个根本问题：

- 没有"官方"实现，方言即分裂
- 内容表达能力弱，扩展靠方言，方言语法混乱
- 引用受路径影响且粒度粗
- 缺少 Agent 原生设计

它在语法与设计上有很多地方受 Rust 和 Typst 启发：Markup/Code 双模式、静态类型与诊断、模块路径、`@` 旁置属性标注等等。

```not
= 部署手册

#callout(kind: "warning", title: [先验证])[
  修改后运行 `notist check`，命令规范见 #<vault::05-cli>。
]

- 条目支持 *强调*、_斜体_ 与行内 `code`
```

## 设计思想

Notist 把文档当作程序：一份 `.not` 是一段程序的求值产物，一个 Vault 是一个由 ModulePath 组织的代码库。上面四个问题的答案都从这一点推出。

### 一切内容是求值的结果

- *糖只是书写层。* `= 标题` 与 `#heading(level: 1)[标题]` 的求值产物是同一个节点；`- 条目`、行内的 *强调* 与 `code`、`#<目标>` 莫不如是。书写层保持轻，语义层保持统一——内容永远是「函数名 + 参数」的 call 森林。
- *扩展即注册规约，且两层对称。* 语义层，插件向求值注册新元素，与内置元素参与同一个规约不动点；投影层，同一份声明对应的 web component 负责呈现。作者写的 call 与呈现无关，呈现是插件在投影端的规约。core 不是特权内核——全部内置内容函数（包括 `text`）都由 core 插件经同一套机制注册。
- *未知即兜底。* 未注册的名字不会被悄悄丢弃：调用保留为内容节点，渲染出一枚可见的兜底标签，诊断由 check 明确指出。扩展的失败模式是可见、可查的，而不是一次构建事故或一段消失的内容。

### 引用是身份，不是路径

- *寻址与磁盘解耦。* `#<vault::guide/install>` 引用的是 ModulePath 加 ItemName 的结构化身份，不是文件路径。移动、重命名、拆分文件，引用面不变。
- *粒度到 Item。* 一个标题链、一个显式 `@id`、一段手动 scope、一个资源文件，都是可引用的一等目标——引用粒度不是「整个文件」。
- *缺席可证明，变更有清单。* 静态引用图让 `notist check` 对每一条引用做存在性裁决，`notist inspect refs` 把「谁在模块外引用了我」枚举成改名/移动/删除的行动项。零命中是证明，不是空结果。

### 标注是旁置的，细到区间

- *属性不进内容流。* `@id`、`#tag`、`key = value` 存进旁置的属性表，绑定到区间上：行内 postfix 标注一句话，块级前缀标注一个节点，模块头标注整篇文档。给一句话打标签是语言能力，不是约定俗成的注释风格。
- *细粒度不破坏结构。* 渲染时区间属性投影为锚点与 `data-notist-*` 钩子；跨段的区间由透明 span 切分包裹，语义树不受影响。查询、样式与工具由此有稳定的挂载点。

### 为两个读者写作

- *同一份结构，两种消费。* 人读渲染出的站点，Agent 读结构本身。CLI 命令面按结构导航设计：`read` / `refs` / `outline` 返回完整结果（无翻页、无截断），坐标是能直接复制进下一个命令的 ItemPath 与 source fingerprint。
- *Agent 无需读源码。* `notist skill` 从二进制内嵌资源生成 Agent Skill，查询、校验、编辑与验证的约定随 CLI 一同分发；文档 Vault 自举——你正在读的这个仓库，文档全部是 `.not`。

文档即程序，Vault 即代码库：内容是求值的值，引用是它的静态检查，属性是它的旁注，而人和 Agent 读的是同一份结构。

## 仓库结构

- `crates/` — Rust 工作区：`notist-syntax`（文法）、`notist-analysis`（分析）、`notist-service`（服务核心）、`notist-cli`（CLI 与 LSP）、`notist-plugin-host` / `notist-plugin-sdk`（插件系统）、`notist-html`（站点产出）、`notist-eval` / `notist-model`
- `plugins/` — 官方插件（core、mermaid、shader 等）
- `docs/` — 用 Notist 自举维护的文档 Vault（`Notist.toml` 为根），*文档一律 `.not`，不使用 `.md`*
- `.agents/skills/` — 面向 Agent 的 notist 使用技能（也可由 `notist skill` 生成）

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
notist inspect read vault::02-cheatsheet --vault docs    # 读取源文并标注生效属性环境
notist inspect refs vault::04-world::reference --vault docs   # 谁在模块外部提及它（改名/移动/删除时的行动清单）
notist check --vault docs                                # 校验模块与引用
```

命令面没有写命令：用任何编辑器修改 `.not` 文件，daemon 通过 watcher 感知变更，改完跑 `notist check` 验证。`notist build` 与 `notist preview` 负责站点产出，`notist lsp` 供编辑器接入（Zed / Obsidian 插件即基于它）。完整命令规范见 `docs/05-cli/README.not`。

## Agent 接入

`notist skill init <dir>` 从二进制内嵌资源生成官方 Agent Skill（单文件 `SKILL.md`，与本仓库 `.agents/skills/notist` 同源），放进你的 Agent 技能目录即可：

```sh
notist skill init .agents/skills/notist
```

Skill 覆盖查询与导航命令、Selector/Scope 寻址、完整结果契约（无翻页、无截断）等约定，Agent 无需阅读源码或文档 Vault 就能正确驱动 CLI。

## 文档

`docs/README.not` 是文档 Vault 的入口：语法参考、内置构造器、类型系统、CLI 规范、设计记录都从那里引用。仓库访客可直接读文件，Agent 与本机用户建议通过 `notist` CLI 查询（`notist inspect read` / `inspect refs` 等，见 `docs/05-cli/README.not`）。

## 相关项目

- [zed-notist](https://github.com/AzurIce/zed-notist) — Zed 编辑器扩展
- [obsidian-notist](https://github.com/AzurIce/obsidian-notist) — Obsidian 插件（独立 Notist World + tree-sitter 高亮 + LSP 客户端）
- [tree-sitter-notist](https://github.com/AzurIce/tree-sitter-notist) — tree-sitter 语法的上游

## 状态

项目处于活跃开发中，从未发布过任何版本；命令面与语法仍会以破坏性方式调整。
