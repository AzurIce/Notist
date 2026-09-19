# Notist

Notist 是一门文档编程语言，使用 .not Markup 和 .notc Code，共享求值器与 Content / Item 模型。

语言核位于 `crates/notist-next`，包含解析、求值、WASM 函数、package 加载和 Content 输出。`notist-analysis` 提供多前端文档与查询，`notist-cli` 提供 stdio LSP、package 检查和浏览器预览。

## 运行

需要支持 edition 2024 的 Rust 工具链。

```sh
cargo test -j8 --workspace -- --test-threads=4
cargo run -p notist-cli -- check examples/workspace
cargo run -p notist-cli -- preview examples/workspace
cargo run -p notist-cli -- lsp
cargo run -p notist-cli -- eval examples/workspace --html
```

安装命令：

```sh
cargo install --path crates/notist-cli
```

Nix 默认包和应用提供 `notist`。浏览器调试器通过 `just web-build`、`just web-fixtures`、`just web-serve` 启动；静态服务使用开发环境自带的 `miniserve`，访问 http://127.0.0.1:8000/app/，可用 `just web-serve 8001` 指定端口。

LSP 支持 `.not` / `.notc` 的语法诊断、嵌套 section 大纲、局部 let 定义跳转与悬停、基础补全，支持 UTF-8 / UTF-16 和增量同步。跨模块符号查询、Markdown 前端、daemon、搜索及编辑器自定义预览协议尚未实现。现有 `docs/` 仍需语法迁移，不能作为 `check` 的验收样例。

`preview` 默认监听 `127.0.0.1:8000`，每秒重新加载 package；可用第二个参数指定地址。`packages/mermaid` 提供真实图表渲染，`packages/canvas` 提供矩形、圆形和文本的 2D 绘图，均使用普通 Notist 函数和浏览器组件。

## 目录

- `crates/notist-next/`：语言与 package 实现，后续按实际职责拆分。
- `crates/notist-analysis/`：文档、位置编码、语法查询与工作区。
- `crates/notist-cli/`：命令入口、LSP 和预览服务。
- `packages/`、`examples/workspace/`：组件 package 与集成示例。
- `crates/notist-next/examples/`：package、WASM 与组件示例。
- `docs/`：设计和讨论记录，使用 .not 文件。
- `archived-docs/`、`corpus/`：历史文档与语料，不能作为当前实现的验收标准。

语言能力和已知范围见 [原型说明](crates/notist-next/README.md)。项目尚未发布，语法和接口仍在开发中。
