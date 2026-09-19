# Notist 原型调试器

运行 `just web-build` 构建浏览器求值器，`just web-fixtures` 同步示例，`just web-serve` 启动本地调试页面。`just web-compare` 对同一 package 请求比较 native 与 WASM 的最终 Content 和诊断。

调试器显示 tokens、AST、普通函数求值事件、Item 内容树和语言诊断。内置 package 示例的请求由 Rust package 加载器生成，携带虚拟源码、WASM 与依赖别名映射。目录选择用于单个源码树；跨目录依赖使用 CLI 生成的 package 请求。

`renderer.js` 是独立的 HTML 消费者。CLI 的 `--bundle` 输出包含内容树、组件资源清单、浏览器挂载代码和组件资源。基础节点生成原生 HTML，扩展组件实现 `render(args, context)`；组件可以通过 `context.renderContent` 挂载嵌套 Content。缺少组件或组件运行失败属于渲染诊断，不改变语言快照。

组件入口由 package 加载器按 `components/<name>.js` 或 `components/<name>/main.js` 自动发现。Notist.toml 无组件声明；同名入口报冲突，其他文件保留为资源，浏览器通过生成的 components.json 加载入口。

`just web-compare` 只比较 native 与浏览器 evaluator 的 `result.content` 和 `result.diagnostics`。Mermaid 与 Shader 示例只展示源码，用于验证资源传递与参数协议。
