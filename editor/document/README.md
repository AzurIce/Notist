# 文档编辑内核

以 Loro 为基础的通用纯文本内核。权威状态只包含文本；语言服务、Tiptap/CM6 投影、presence、网络连接和持久化策略由宿主实现。

## 组成

- `../crates/editor-document`：Rust `Document`，native/Wasm 共用同一实现。
- `../crates/editor-node-wasm`：Document 和 Node 共用的 Wasm 绑定，不依赖 Notist 语言 crate。
- `index.mjs`：JS 宿主接口，负责对象转换与安全的异步变更通知。
- `../experiments/source-projection`：接入内核的 Tiptap/CM6 双视图实验。

## 构建与验证

在仓库开发环境中，从仓库根执行：

```sh
just editor-document-build
just editor-document-test
```

需要 Rust 的 `wasm32-unknown-unknown` target、Wasm linker、wasm-bindgen-cli 0.2.121 和 Bun。构建输出在 `editor/pkg-editor/` 和 `editor/scripts/pkg-editor-node/`。Rust 依赖固定 Loro 1.16.2；Cargo.lock 固定传递依赖。

单独验证 native：

```sh
cargo test --manifest-path editor/Cargo.toml -p notist-editor-document
```

双视图实验还需要现有语言核的 `just web-build` 产物：

```sh
cd editor/experiments/source-projection
bun install --frozen-lockfile
bun run build
bun run test:browser
bun run dev
```

## 使用

浏览器宿主自行提供生成的 Wasm 文件 URL：

```js
import init, { DocumentBinding } from "../pkg-editor/notist_editor_node_wasm.js";
import { EditorDocument } from "./index.mjs";

await init();
const core = EditorDocument.create(DocumentBinding, {
  identity: { document_id: "draft", history_id: crypto.randomUUID() },
  text: "Hello 🧠",
});

const unsubscribe = core.subscribe(event => {
  // event.edits uses BEFORE-text UTF-16 offsets.
  // event.after is an immutable snapshot with its causal version.
  render(event.after);
});

const before = core.snapshot();
core.transact({
  expectedVersion: before.version,
  origin: "source-view",
  edits: [{ from: 6, to: 8, insert: "world" }],
  undoMetadata: { view: "source", selections: [[6, 8]] },
  undoPositions: [6, 8],
});

core.undo({ view: "source" }, [6, 11]);
unsubscribe();
core.dispose();
```

## 接口契约

### 状态、身份与版本

一个 `Document` 对应一份文本。`document_id` 是宿主提供的稳定文档身份，`history_id` 区分独立历史。仅有相同路径或相同文本不代表相同历史。创建历史只做一次；其他副本从其快照恢复。

写入者身份在一个实例的生命周期中固定，默认由 Loro 生成。可显式提供十进制 `writer` 字符串，宿主必须保证不同活跃实例不复用。快照恢复要求新写入者，不允许选择已经在历史中写入过的 peer。导入时还会拒绝声称包含该实例写入者的新操作。

`snapshot()` 返回自有且不可变的文本快照。`version` 是包含文档/历史身份和因果时钟的跨副本状态标识；`revision` 是当前实例的通知序号，不能作为跨实例版本。快照恢复后 revision 从零开始。

### 事务与坐标

所有修改提交到 `transact`。调用者必须携带生成修改时的 `expectedVersion`；过期状态返回 `stale_version`，由上层决定重算或重新定位。源码无需满足任何语言语法。

编辑区间为 UTF-16 的 `[from, to)`，全部参照修改前的同一个文本，按起点递增排列，不能重叠或拥有相同起点。禁止拆开代理对；允许组合字符内部的 Unicode scalar 边界。全部区间先校验，再逆序应用并一次提交。无效请求不会留下前半个编辑；没有变化的事务不产生历史和事件。

### 通知

本地编辑、远端合并和撤销重做共享 `ChangeEvent`，其中包含来源、前后快照、显示用文本修改和撤销状态。本地事件保留提交的多个修改区间；导入/撤销的显示修改目前用公共前后缀计算一个连续区间，绝不把这个显示差分再次写入 CRDT。纯因果状态变化可以产生空 edits 的事件。重复包不产生虚假的文档变化。

Rust 通过独立队列向每个订阅者发送提交完成的事件。JS 在微任务里按顺序通知，避免进入 CM6/ProseMirror 或 Wasm 尚在执行的事务。订阅后的新事件才会交付；取消订阅即停止交付。监听器失败通过 `onListenerError` 报告，不把已经完成的写入伪装成失败。监听器内直接写入会被拒绝；需要响应式写入时另行安排任务。事件携带提交时的快照，调用 `snapshot()` 则可能读到更晚已提交的状态。

### 更新交换与恢复

`exportSnapshot()` 导出带身份的完整 CRDT 历史包。`exportUpdatesSince(version)` 导出缺失操作，`import(packet, origin)` 处理重复和乱序交付。接收方发现不同文档或历史时拒绝导入。载荷先在临时副本中解码验证，再导入活跃实例。

与现有 Loro 同步实现互操作时，可用 `encodedVersion()` / `decodeVersion(bytes)` 转换原始二进制版本，使用 `importBinary(identity, bytes, origin)` 包装并导入原始快照或增量。原始 Loro 字节不含宿主的文档/历史身份，适配器必须通过房间或握手先确认身份；这些入口仍执行原有导入校验。Rust 对应 `Version::encode/decode` 和 `SyncPacket::from_binary`。官方 WebSocket 实验见 `../experiments/loro-sync`。

因果依赖尚未满足时，`import` 返回 `pending: true`，操作会在内存中等待前序包。宿主应保留未满足依赖的包；本接口不承诺导出的快照持久化等待队列。网络连接、重试、授权、确认和可靠落盘由宿主定义。包的身份封装用于防止误接，不构成认证；只交换兼容内核产生的包。

### 撤销和锚点

默认每个本地事务是一项撤销；`beginUndoGroup/endUndoGroup` 提供明确分组。接受导入会结束当前分组，撤销/重做也会结束分组。初始内容及远端操作不会进入本地撤销栈；撤销保留对方修改。最多保留 500 项本地撤销，快照不持久化撤销栈。

`undoMetadata` 是不参与复制的宿主 JSON 数据，可保存视图信息或主选区编号。撤销事件的 `restored_metadata` 返回对应数据；内核不理解选区结构。`undoPositions` 是另行提交的 UTF-16 位置列表，所有位置都先校验，再交给 Loro 的撤销元数据机制进行协作变换；撤销事件的 `restored_positions` 返回修改后文本中的位置。它支持多个选区端点，也能恢复被替换文字内部的原位置。调用 undo/redo 时可同时传当前元数据和位置，供反向操作恢复。Rust 使用 `undo_with_context/redo_with_context`。

撤销位置使用 Loro 的变换边界：恰好在该位置发生的远端插入留在位置右侧；绑定文档末尾的位置随末尾移动。清空撤销史，以及没有文本变化但消费了历史项的撤销，也会产生通知以更新宿主的撤销状态。

`anchorAt(offset, "before" | "after")` 明确描述同一空隙插入文字时锚点位于插入内容的哪侧，分别绑定前/后字符；文本两端使用边界锚点。目标被删除时，锚点收敛到保留历史所定位的删除空隙。`resolveAnchor` 返回 UTF-16 下标和可重新保存的锚点。锚点包含文档/历史身份，可在同历史副本之间传递。普通锚点采用删除后折叠的语义，不能替代 undoPositions 来恢复替换前的选区。

当前保留完整历史，拒绝 shallow snapshot，也不暴露裁剪或 checkout。持久化和网络同步由 `../node` 的 `EditorNode` 提供。大文档增量投影、事件快照复制成本及跨文档事务不在 Document 的职责内。
