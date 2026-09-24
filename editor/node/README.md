# Editor Node

`editor-document` 持有 Loro 纯文本权威状态；`editor-node` 管理文档、对等同步和持久化。UI、Notist 语言服务和 LSP 均不属于这两层。

```text
Tiptap / CM6 / Agent              native editor / headless bin
        │                                    │
        └────────── EditorDocument ───────────┘
                         │ 同一个 Document handle
                     PeerNode
                  /             \
          Storage effects      Peer messages
         IndexedDB / redb    WebRTC / WebSocket

native NodeRuntime 可选 network_service = 信令 + STUN/TURN
native NodeRuntime 可选 peer = 常驻文档副本 + redb
                            + 按文档配置的纯文本输出
```

## 组成与构建

- `../crates/editor-document`：无网络依赖的 Rust 文档内核。
- `../crates/editor-node`：无 I/O 的 `PeerNode` 状态机；`native` feature 加入运行时及单个 `notist-editor-node` binary。
- `../crates/editor-node-wasm`：同一 Wasm 模块导出 `DocumentBinding` 与 `NodeBinding`，共享 Rust 文档 handle。
- 本目录的 `index.mjs` / `store.mjs`：浏览器 I/O 宿主；浏览器使用原生 WebRTC/WebSocket，存储使用 IndexedDB。
- `client.mjs`：现有 source-projection 实验的接入适配器。

仓库根运行：

```sh
just editor-node-build
just editor-node-test
just editor-node-run editor/node/examples/local.toml
```

构建需要当前 Rust toolchain、`wasm32-unknown-unknown`、Wasm linker、wasm-bindgen-cli 0.2.121 和 Bun。浏览器测试需要 openssl 和 Chrome；可用 `CHROME_PATH` 指定可执行文件。

也可只构建、部署 native 单 binary：

```sh
cargo build --release --manifest-path editor/Cargo.toml -p notist-editor-node --features native
editor/target/release/notist-editor-node --config node.toml --check
editor/target/release/notist-editor-node --config node.toml
```

`--check` 检查配置结构与约束；实际启动还检查证书与端口绑定。相对路径相对于配置文件目录。启动全部 listener 后输出一行 `ready` JSON，SIGINT/SIGTERM 关闭连接并排空存储队列。运行时致命存储错误停止进程，不能继续报告保存成功。

## 按需组合

| 配置 | 行为 |
| --- | --- |
| `peer.enabled=true`，`network_service.enabled=false` | 持久化文档节点，提供 `/peer` WebSocket；也可连外部信令使用 WebRTC |
| `peer.enabled=false`，`network_service.enabled=true` | 信令 + STUN/TURN；只保存节点身份，不打开文档数据库 |
| 两者均启用 | 一个进程提供全部能力；native PeerNode 直接加入进程内信令注册表 |

`network_service` 是一个开关，信令和 TURN 共享服务凭据、临时 ICE 凭据签发和进程生命周期。内部仍按协议拆分模块与端口。TURN 使用 Rust `turn` crate，TCP/TLS framing 由适配器接入，不启动 coturn 或其他进程。TCP/TLS 指客户端到 TURN 服务的传输；中继端为 UDP allocation。

`examples/local.toml` 适用于本机开发。`examples/public.toml` 展示纯网络服务部署，需填写真实公网 IP、域名、随机凭据和 TLS 证书。公网配置需开放 HTTP(S)、TURN UDP/TCP、TURN TLS，以及配置的 UDP relay 端口范围；普通 HTTP 反向代理不能代理 TURN。若经过 NAT，监听地址是本机地址，`public_ip` 是实际映射的外部地址。明文 HTTP 仅允许监听 loopback。

`peer.connect` 配置 native WebSocket 邻居；`peer.signaling` 配置外部信令节点。配置中身份相同的文档通过双向增量交换收敛。**初始内容只由创建者插入一次**；其他 native 副本使用相同 `document_id`、`history_id`、文档凭据和空 `initial_text`，再通过同步取得历史。浏览器首次加入可使用已存在的完整快照。不要在多个新副本各自重复插入初始内容。

## 浏览器接入

```js
import init, { DocumentBinding, NodeBinding } from "../pkg-editor/notist_editor_node_wasm.js";
import { EditorNode, IndexedDbStore, connectSignaling } from "./index.mjs";

await init();
const store = await IndexedDbStore.open("my-device-profile");
const node = new EditorNode({ DocumentBinding, NodeBinding, store });
const document = await node.openDocument({ identity: seed.identity, seed, credential });
const connection = connectSignaling(node, { url: "wss://node.example/signal", token });

// Bind both editors to this same document; its API is editor/document/index.mjs.
document.subscribe(event => render(event.after));
await node.flush(); // Await local journal commit, not a remote replica receipt.
node.remoteDurableVersions(seed.identity.document_id); // Remote persisted versions.

connection.close();
await node.close(); // Owns/free the managed documents and releases the profile lock.
```

一个 IndexedDB profile 只允许一个活跃页面，避免并发恢复同一节点。不同副本用不同 profile。节点身份稳定，进程/页面会话和恢复后的 Loro writer 都重新生成。`MemoryStore` 只用于易失性节点，不会报告 durable version。

现有 Tiptap/CM6 原型的接入 URL 见 `../experiments/source-projection/README.md`。浏览器无法运行 server listener；它只携带 document、peer 状态机、存储和客户端 transport。

## 同步、鉴权与保存

`PeerNode` 不进行 socket/文件操作。宿主提供连接并输入 `WireMessage`，排空 `Effect::Send` 和 `Effect::Persist`；编辑 handle 后调用 `poll()`，完成真正持久化后调用 `persisted()`。导入必须通过 `PeerNode::import`（JS managed document 已自动路由），使尚未满足因果依赖的包也进入 journal。

连接先交换节点/会话身份。文档凭据生成绑定双方会话及文档历史的 HMAC 证明，不在 peer 握手中发送凭据原文。双方授权后广播应用版本及持久化版本，根据因果时钟请求缺失操作。本地、导入、undo/redo 都走同一变更订阅，所以 A→B→C 可传播；重复更新不形成回声循环。重连重新比较版本，旧会话消息被拒绝。更新分片并限制每条连接的重组大小；浏览器与 native 发送队列均有限制。

网络服务使用独立 `access_token`，WebSocket 首帧认证；HTTP 使用 Bearer。每份文档还需要自己的 credential。信令注册回复短期 TURN 凭据，客户端定期续签并刷新 ICE。共享服务 token 属于同一受信组；节点 UUID 是路由身份，不是公钥身份。信令服务和同组节点目前按受信环境部署；这些证明不等于独立于传输层的端到端身份认证或内容加密。当前没有用户账户、读写分权、签名成员表或自动密钥分发。

每次存储回执覆盖提交时捕获的版本，不会把之后的编辑误报为已经持久化。redb 事务和 IndexedDB 完成事件之后才发 durable receipt；网络发送、接收、内存合并分别不等于落盘。恢复时顺序重放 journal（含 pending 包），校验每条 applied version，并使用新 writer。浏览器 durable 表示 IndexedDB 提交，仍受浏览器站点存储清理策略约束。

## Native 纯文本输出

每个 `[[peer.documents]]` 可配置 `text_file = "../notes/note.not"`，路径相对于配置文件目录；不配置则只保存 CRDT 数据库。本机示例已配置输出到仓库内的 `editor/node/data/local/note.not`，启动 `just editor-node-run editor/node/examples/local.toml` 后即可用其他编辑器查看。父目录自动创建，文件内容保留完整源码、Unicode 和原有换行，不做格式化。

方向只有 **Document → 已提交的 redb journal → 文本文件**。本地编辑、远端合并及撤销重做使用相同路径，通常以约 100 ms 的间隔合并写入。每次通过同目录临时文件与原子替换发布，正常退出前排空队列。重启以数据库为准，补写缺失或落后的输出；数据库还保留替换前后的内容摘要，用于识别中途崩溃留下的文件。

文件修改不会导入 CRDT，也不会作为首次创建文档的输入。已有文件内容不符或后来被其他编辑器修改时，输出进入 `conflict`，保留外部内容；符号链接和非普通文件进入 `error`。将冲突文件另存或移走后，节点会自动重试生成。检查是写入前校验并定期复查，不能与不受控的外部写入形成跨进程原子锁；这些文件适合作为查看输出。

`NodeRuntime::text_files()`、启动的 `ready` JSON 和 `/health` 中的 `text_files` 提供 `pending / synced / conflict / error` 状态及最后输出的 CRDT 版本；错误详情写入日志。文本输出故障不影响 CRDT journal 提交和对等同步。**durable receipt 只确认数据库提交**，文本文件有独立的输出状态。

当前保留完整历史和追加 journal，没有压缩。撤销栈不跨重启保存。同步帧上限 64 KiB，单包重组上限 16 MiB，每个节点最多 64 份打开文档、32 条 peer 连接。适合继续开发和互操作验证；大文档、长期 journal、真实跨公网 NAT、Safari/Firefox、带宽配额与对抗性负载仍需专项验证。

## 验证

`just editor-node-test` 运行 Rust 状态机/运行时与 JS/Wasm 契约、真实 Chromium 集成。覆盖：

- 三节点转发、环路静止、断线并发编辑再合并、Unicode 分片、协作撤销。
- 文档鉴权、会话绑定、存储背压、易失存储、乱序 pending 包重启恢复。
- 单进程 relay-only 下强制 TURN UDP/TCP/TLS，并检查实际选中的 relay candidate。
- native/browser WebRTC、外部信令节点、native WebSocket 链式同步。
- Tiptap/CM6 双页面编辑和撤销、IndexedDB 重载、native SIGKILL 后恢复已确认写入。
- 纯文本输出的持久化顺序、关闭排空、缺失文件恢复、原子替换中断恢复和外部修改保护。

这些网络测试运行在本机；它们验证协议互通和持久化路径，不代表所有公网 NAT 环境均已覆盖。
