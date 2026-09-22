# Notist Editor

Web 编辑器应用与 Wasm 接入，当前界面为语言调试器。`app/` 提供浏览器界面，`crates/editor-wasm/` 中的 `notist-editor` crate 调用 Notist 语言栈的调试快照 API。`editor/` 是独立 Cargo workspace，固定 wasm-bindgen 版本和 Wasm release profile。

在仓库根运行 `just web-build` 构建浏览器求值器，`just web-fixtures` 同步示例，`just web-compare` 对同一 package 请求比较 native 与 WASM 的原始 Content、成型 Content 和诊断。需要本地服务时，手动运行 `just web-serve`，访问 http://127.0.0.1:8000/app/。

开发阶段和验收标准见 [ROADMAP.md](ROADMAP.md)。

调试器显示 tokens、AST、普通函数求值事件、原始及成型 Item 树和语言诊断。内置 package 示例的请求由 Rust package 加载器生成，携带虚拟源码、WASM 与依赖别名映射。目录选择用于单个源码树；跨目录依赖使用 CLI 生成的 package 请求。

`just web-compare` 比较 `evaluation.content`、`result.content` 和 `result.diagnostics`。Mermaid 与 Shader 示例只展示源码，用于验证资源传递与参数协议。
