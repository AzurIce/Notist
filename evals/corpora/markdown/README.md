---
kind: reference
status: current
---

<a id="notist"></a>
# Notist

#[

**Note + ist -> notist(also not an ist)**

Notist，一门带有静态类型系统的文档编程语言，它为取代 Markdown 而生，它的目的是解决 Markdown 存在的诸多问题：
- 没有核心的“官方”实现
- 内容表达能力弱，扩展靠方言，各种方言语法混乱
- 引用受路径影响且粒度粗
- 缺少 Agent 原生设计

它在语法和设计上有很多地方受 Rust 和 Typst 启发。

] <!-- @ann: type=user,.user -->



.not

- Notist 介绍与核心概念 [intro](intro.md)
- 语法参考：Markup/Code 双模式与核心文法 [grammar](grammar.md)
- 内置构造器：14 个 built-in 的签名与示例 [functions](functions.md)（语言规范见 [core](designs/plugin-system/core.md) 与 [syntax-sugar](designs/language/syntax-sugar.md)）
- 类型系统与求值模型 [types](types.md)
- 日常语法速查 [cheatsheet](cheatsheet.md)
- 命令行完整规范（设计 + 使用）[cli](cli.md)
- 插件系统示例：Shader [plugins](plugins.md)
- AI 的一些调查整理 `<ai>`
- 设计记录 `<vault::designs>`
