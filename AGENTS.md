本仓库的 `docs/` 下是文档目录，注意不要出现 `.md` 都使用我们的 `.not`。

当用户要求整理文档到 `docs/` 下时，应该整理到 `docs/ai` 下，以 `yyyy-mm-dd xxx` 命名。
整理后要求在 `docs/ai/README.not` 中进行引用，且附带一段简短的摘要，方便后续 Agent 查找复用。

本项目从未发布过任何版本：不要用 v1/v2/v3 这类版本号命名或引用迭代。需要引用某个历史状态时，用日期（如"2026-08-15 裁决的命令面"）或 commit hash。第三方产品自身的版本号（如 Typst 0.14）不受此限。

## 姊妹项目

仓库根的 `.env`（已 gitignore）声明了四个相邻项目的本地路径，跨仓库引用时从这里取：
- `ZED_NOTIST_PATH`：Zed 编辑器扩展（grammar 快照 + queries + LSP language server 声明）
- `OBSIDIAN_NOTIST_PATH`：Obsidian 插件（tree-sitter 高亮 + 自研 LSP 客户端 `src/lsp/`）
- `TREE_SITTER_NOTIST_PATH`：tree-sitter 语法上游（grammar.js + queries）
- `VSCODE_NOTIST_PATH`：VS Code 扩展（TextMate grammar + vscode-languageclient 客户端 + renderDocument 预览）

## 核心变动的跨项目审查

notist 是这些项目的上游。当本仓库发生下列核心变动时，必须同时审查各姊妹项目的设计与实现影响，不要只凭记忆判断"应该没关系"：

- **语法/文法变更**（notist-syntax、新构造器、解析规则）→ tree-sitter-notist 的 grammar 与 queries 需同步；zed-notist 内嵌 grammar 快照（extension.toml 钉 rev）需升级；obsidian-notist 高亮依赖同一产物；vscode-notist 的 TextMate grammar 是移植近似，需对照其 `scripts/fixtures/sample.not` 与 `just tm-smoke` 更新。
- **LSP 协议契约变更**（capabilities、FULL sync 规则、诊断推送模型、错误码、方法集、编码协商）→ obsidian-notist 的 `src/lsp/session.ts` 头注释逐条记录了它依赖的服务端契约，需核对更新；vscode-notist 的 `src/protocol.ts` 头注释同理（可用其 `just lsp-smoke` 对真实 server 回归）；zed-notist 的 language server 接入同理。
- **分析/诊断语义、CLI 面、插件 ABI 变更** → 按需检查各项目的调用点与文档。

审查方式建议并行派子代理（explore 或 general），每个项目一个：输入为本次变动的摘要与相关文件清单，要求返回"受影响的设计文档 + 代码位置"清单；汇总后再决定跟进修复。涉及 LSP 客户端行为假设的改动，还应实机回归（如经 obsidian CLI 驱动验证）。
