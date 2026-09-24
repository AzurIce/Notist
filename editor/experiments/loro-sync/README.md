# Loro 官方同步实现接入实验

使用发布的 `loro-websocket` 客户端和 `SimpleServer`，让两个现有 Tiptap/CM6 编辑器通过真实 WebSocket 协作。客户端唯一文档状态仍是 Rust/Wasm `EditorDocument`。本实验用于验证接入与暴露参考实现的边界，不代表同步服务已经适合生产部署。

## 运行

先在仓库开发环境构建内核与现有原型：

```sh
just editor-document-build
# 语言核尚未构建时还需 just web-build
cd editor/experiments/source-projection
bun install --frozen-lockfile
bun run build
cd ../loro-sync
bun install --frozen-lockfile
bun run dev
```

访问 <http://127.0.0.1:4175>，页面并排展示两个独立副本。每个副本都包含 Tiptap 和 CM6，并有「模拟断网 / 恢复连接」按钮。「单独打开」会创建额外副本，避免多个页面覆盖同一个离线存储槽。也可以用 `/?room=another-room` 开一个新的实验文档。

HTTP 默认 4175、WebSocket 默认 8787，都只监听 `127.0.0.1`。可通过 `PORT`、`WS_PORT`、`SYNC_DATA_DIR` 配置。服务端定期快照默认写入被忽略的 `data/` 目录；客户端 CRDT 快照写入 IndexedDB，按房间和副本隔离。支持 Web Locks 的浏览器会拒绝同时打开同一个副本存储槽。原来 4173 的独立草稿及 localStorage 不受影响。

可实际尝试：两边同时输入；在 A 加粗、在 B 编辑源码，再撤销 A 的格式；两边断网后分别输入，刷新其中一个副本，再恢复连接。刷新保留文档和历史身份，恢复时分配新的写入者；本地撤销栈不持久化。

## 接入关系

```text
Tiptap / CodeMirror
        ↓
Rust/Wasm EditorDocument ← KernelAdaptor ← 官方 LoroWebsocketClient
        ↓                                    ↕ WebSocket
IndexedDB（宿主订阅）                    官方 SimpleServer
                                              ↓
                                       定期快照保存钩子
```

- `adaptor.mjs` 实现官方 `CrdtDocAdaptor` 接口，将二进制版本与更新交给现有内核；没有另外创建客户端 `LoroDoc`。连接握手双向补齐，后续本地编辑、undo/redo 发送增量；接收的网络更新不重复广播。
- `connection.mjs` 使用官方客户端的房间、重连、协议编码和分片能力。等连接建立后再 join。只为未处理的连接 Promise 增加处理器，保留显式调用者可观察的失败。
- `client.mjs` / `store.mjs` 管理原型的生命周期和 IndexedDB。待满足因果依赖的原始包另外保留，恢复时重放；实验尚未压缩这份 pending 记录。页面区分本地保存与网络连接状态。
- `server.mjs` 直接实例化官方 `SimpleServer`，通过公开的加载/保存钩子接文件存储；文档身份在 bootstrap 时创建一次，room 和 join 元数据绑定该身份。身份检查只防误接，不是用户认证。原始更新包仍按兼容客户端之间的实验流量处理。
- 内核只增加 Loro 二进制版本编码、解码和原始更新包装接口。身份校验、完整历史约束、写入者检查和原子导入校验仍经过原来的 `Document::import`，没有网络或 LSP 依赖。

服务器保存文件采用临时文件加 rename；没有提供断电级 fsync 保证。浏览器没有离线应用壳：页面加载仍需要本机 HTTP 服务，「离线」验证针对同步连接。

## 实测结果与问题

固定依赖：`loro-websocket` 0.6.2、`loro-protocol` 0.3.0、`loro-adaptors` 0.6.1，客户端 Rust Loro 和服务端 npm Loro 均为 1.16.2。参考服务端会静态加载 Flock 适配器，因此还需安装其声明为 optional 的 `@loro-dev/flock` 4.5.1；它不进入浏览器同步 bundle。

验证通过的行为：三副本并发编辑收敛；双端离线修改后重连双向补齐；本地撤销保留远端修改；恢复离线快照后上传未发送操作；已完成定期保存的服务端快照可在重启后恢复；同一内核的 Tiptap/CM6 视图随远端编辑刷新。

同时通过可复现测试确认了两个重要边界：

1. **成功 ACK 早于持久化。** 接收方内存里接受更新后就确认、广播，保存是独立的定时任务。实验界面的「已连接」不表示服务端已保存。
2. **参考服务端有异步保存竞争。** `saveAllDirtyDocuments()` 等待保存回调时，新的更新可能把 `dirty` 设为 true；旧保存完成后又无条件设为 false。测试在保存回调中设置屏障，期间提交并收到第二项修改的 ACK，释放屏障后经过两次保存间隔，磁盘快照仍缺第二项修改。测试验证问题存在；没有修改 vendor 掩盖它。

另外，官方客户端构造函数没有处理内部 `connect()` 返回的异步 Promise；初次连接尚未完成就关闭会出现未处理拒绝。实验用一个很小的派生类给该 Promise 安装 catch，仍把原 Promise 返回给显式调用者。离线刷新浏览器回归覆盖这个路径。

因此，官方客户端与协议能够接入当前内核；参考服务端的存储路径需要修复与进一步验证，不能直接把 ACK 当作远端可靠保存。本实验没有实现持久化确认协议、历史裁剪、跨文档同步或生产鉴权。

来源：[官方仓库](https://github.com/loro-dev/protocol)、[线协议](https://github.com/loro-dev/protocol/blob/1f8a0fa07bab5320ae154ae7cfa4add8b3b8c4fc/protocol.md)。结论以安装的固定发布包及本目录测试为准。

## 验证

```sh
bun run test
bun run build
bun run test:browser
```

8 项网络/适配器测试包括上述 ACK 语义和保存竞争复现；6 项浏览器测试覆盖真实双视图协作、跨副本撤销、离线刷新、重连补齐、服务端保存后重启和演示页。测试使用临时服务、临时文件目录及独立浏览器上下文，不修改预览草稿。Chrome 路径可通过 `CHROME_PATH` 指定。浏览器报告和截图在被忽略的 `results/`。

原型回归仍由 `../source-projection` 的 `bun run test` 和 `bun run test:browser` 运行；内核检查使用仓库根的 `just editor-document-test`。
