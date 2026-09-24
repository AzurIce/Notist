# Tiptap / Notist 源码投影实验

一个可以实际编辑的双视图原型，用来验证「Tiptap 文档体验 + 纯文本源码作为唯一原件」是否可行。它独立于现有语言调试器；不代表正式编辑器架构已定案。

## 运行

需要 Bun，以及仓库当前文档内核和语言核的浏览器构建。在仓库根运行 `just editor-document-build`；若 `editor/pkg/notist_editor_bg.wasm` 不存在或语言核有变动，再执行 `just web-build`。

```sh
cd editor/experiments/source-projection
bun install --frozen-lockfile
bun run dev
```

访问 <http://127.0.0.1:4173>。可通过 `PORT` 环境变量修改端口。服务只监听本机。

需要体验两个副本协作时，运行 `../loro-sync` 的官方 WebSocket 接入实验，访问其 4175 演示页。它复用本原型的视图，通过可选会话入口提供网络连接和 CRDT 历史恢复。

## 可以尝试

- 在文档中直接输入、选中文字加粗/斜体、切换二级标题、Enter 分段、Backspace 合并段落。
- 在右侧直接编辑完整 `.not` 源码；正文和格式会重新投影到左侧。
- 交替在左右编辑，再用 ⌘Z / ⇧⌘Z 撤销、重做；两边使用同一个 Loro UndoManager。
- 输入不完整的 `#let x = (`，查看语言核诊断；继续编辑其他段落，不完整源码仍会保留。
- 点击源码块的「在源码中编辑」，或点击诊断，定位到右侧对应范围。
- 切换成仅文档视图；导出 `.not`。草稿以纯文本写入当前浏览器的 localStorage。

## 实现和边界

权威文本由独立 Rust `editor-document` 中的 LoroText 持有，通过独立 Wasm 绑定和 `editor/document/index.mjs` 的宿主接口访问。`app.js` 仅作为视图、投影和语言服务适配层，不直接操作 Loro。Tiptap/ProseMirror 文档是派生投影，CodeMirror 展示完整文本。两套编辑器自身的撤销历史都被禁用；撤销统一提交给内核，再同步视图。内核的变更订阅也接收 Agent 式外部写入与远端导入；独立运行时不连接网络，网络模式可接入 `../node` 的 Rust 节点；原有 `?room` 模式仍用于 `../loro-sync` 互操作实验。没有历史裁剪或持久化撤销历史。

`projection.mjs` 是保守的实验适配器，不是 Notist 解析器。它支持单行段落、三级以内标题、加粗和斜体，以及有限的字符转义。带表达式、注解、注释、代码、数学、多行段落或不能精确往返的语法会变为受保护的源码块；对应原文、空行、缩进和其他未修改块保持不变。源码块只能在完整源码视图里修改，文档里的跨块删除会被阻止并提示。未闭合的复杂构造保守地保护后续源码。文档视图是编辑投影，并非完整的语言求值渲染。

富文本事务先生成候选源码，复用原始块边界的空白和源码块，再通过公共前缀/后缀差分提交一个连续替换区间。未修改字符保持原样，但这仍不是面向多人协作的生产级区间映射：一次跨段格式操作可能覆盖较大的替换范围。后续若选择这条路线，应接入语言核/CST 的源区间，并设计按编辑操作产生的多个局部补丁、稳定锚点和远端合并规则。

文档自身输入时不重建 Tiptap 文档，仅更新必要的块身份属性，避免打断输入法。源码编辑时会重新生成整个文档投影。适配层把撤销选区端点提交为内核的 undoPositions，在撤销时读取协作变换后的端点；视图身份作为不透明元数据保存，内核不理解视图或选区结构。

语言诊断由 `language-worker.js` 调用仓库 `editor/pkg/` 中的 Rust/Wasm 语言核产生，使用请求修订号丢弃旧响应，并把 UTF-8 字节区间转换为编辑器的 UTF-16 范围。这次接入的是实际语法/求值诊断，**还没有实现 LSP 传输、补全、hover 或跳转定义**。源码着色只是演示用词法着色，也不是完整语言高亮。

## 验证

```sh
bun run test
bun run build
bun run test:browser
```

浏览器测试默认使用 macOS 的 Google Chrome，也可通过 `CHROME_PATH` 指定可执行文件。测试启动临时本机服务，使用独立浏览器上下文，不修改正常预览里的草稿。

验证环境为 Tiptap 3.31.3、CodeMirror 6、Rust Loro 1.16.2、Chrome 153。7 项投影模型测试覆盖原文往返、复杂语法保护、局部修改、分段身份、字面量转义和 Unicode 区间；浏览器回归覆盖真实诊断、格式操作、段落编辑、未完成源码、跨视图撤销/重做、字面源码字符、中文组合输入、源码块保护、诊断定位、视图切换、草稿恢复，以及外部事务和远端导入。独立内核的契约和 native/Wasm 对拍见 `editor/document/README.md`。

中文输入验证通过 Chromium CDP 模拟 composition；尚未覆盖所有平台的真实输入法。截图与浏览器结果输出在被 gitignore 的 `results/`。

## 连接 Rust 节点

按 `../../node/README.md` 启动节点，页面通过查询参数选择节点与本机副本：

```text
http://127.0.0.1:4173/?node=http://127.0.0.1:8788&document=note&replica=alice#token=DEV-ACCESS-TOKEN-CHANGE-BEFORE-DEPLOY&credential=DEV-DOCUMENT-TOKEN-CHANGE-BEFORE-DEPLOY
```

默认使用 WebRTC，`transport=websocket` 可直连节点的 `/peer`。凭据位于 fragment，不进入静态服务器请求路径。不同页面使用不同 `replica`，同一持久化 profile 通过 Web Locks 防止同时打开。首次打开从 native PeerNode 获取快照，之后可从 IndexedDB 离线恢复。Tiptap 和 CM6 继续修改同一个 `EditorDocument`；`EditorNode` 负责复制与存储。
