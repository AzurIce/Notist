# Loro 选型验证

独立、可重复运行的实验。这里的结论只描述所列版本和用例；没有改动语言栈、正式 editor 或路线图。

## 环境

- Rust `loro = 1.16.2`；Node/Wasm `loro-crdt = 1.16.2`。
- 另以 Node/Wasm `loro-crdt = 1.16.3` 复核浏览器问题与旧锚点 panic。
- 官方 `loro-codemirror = 0.3.3`，CM6 state 6.7.5、view 6.43.12、commands 6.11.1。
- 实测 macOS arm64、Chrome 153.0.8010.53、Node 24.19.0；依赖由各自 lockfile 固定。

## 结果

| 范围 | 观察 |
| --- | --- |
| 合并与恢复 | 32 个固定随机种子，3 个副本，4,800 次编辑；每次本地编辑先与普通字符串实现比较。乱序、重复交付后文本与版本一致；从基础快照加乱序日志恢复也一致 |
| 跨端 | native/Wasm 对同一操作计划逐步比较，1,063 个步骤一致；覆盖中文、非 BMP 字符、组合字符、并发替换、撤销、锚点；快照双向导入并继续编辑通过 |
| 协作撤销 | 本地插入 `abc` 后，远端在中间插入 `中😀`，撤销本地操作保留 `中😀`，重做恢复合并结果 |
| 写入者与恢复 | 切换 peer 清空既有撤销栈；快照保存文本和历史，但重新创建 UndoManager 不恢复其撤销栈 |
| 锚点 | 保留完整历史时，删除目标后的定位和编码往返通过；返回的新 cursor 可以重新保存 |
| 裁剪后的旧离线副本 | 完整历史可以合并旧副本；shallow snapshot 拒绝依赖被裁剪历史的更新。裁剪点之后的新副本仍能同步 |
| 裁剪后的旧锚点 | 查询已删除字符的旧 cursor 会 panic。Wasm 1.16.2、1.16.3 及 native 1.16.2 均复现；裁剪前刷新后的 cursor 在所测例子中可用 |
| 普通浏览器编辑 | 预载文档的中文、emoji 输入和双副本同步通过 |
| 组合输入 | Chromium CDP 驱动 `ni → 你 → 你好`，期间远端插入 `远`，两端得到 `远A你好B`。一次撤销只退到 `远A你B`，组合输入被拆分 |

`core.test.mjs` 的 8 项常规检查通过，另有 1 项显式确认旧锚点 panic 的局限检查。后者在隔离子进程中运行，不应解读为该问题已修复。

两版 Loro 的浏览器用例均为 6 项通过、6 项失败。失败是该实验要暴露的现状，`browser.test.mjs` 因此返回非零状态；没有修改依赖来隐藏失败。

## 官方 CM6 绑定的接入问题

1. **首次事务存在初始化竞态。** CM6 与 CRDT 初值相同时，初始化微任务仍设置 `isInitDispatch`，随后直接返回。若下一次更新是文档编辑，该编辑被跳过。布局更新可能先清除标志，所以普通首次按键的复现受时序影响；实验固定在初始化微任务后、布局更新前提交事务，稳定复现 `abc中` 与 CRDT `abc` 分离。源位置：依赖的 `dist/sync.js`，`LoroSyncPluginValue` 初始化与 `update`。
2. **官方撤销命令发生嵌套 dispatch。** 命令先向 CM6 dispatch effect；StateField 更新调用 UndoManager，后者的同步回调再次 dispatch 文本变更。实测产生视图异常或 CM6 与 CRDT 文本不一致。对照用例从 CM6 更新流程外直接调用同一个 UndoManager 后，文字、远端修改及单选区恢复正常。源位置：`dist/undo.js` 的 `undoManagerStateField`、`UndoPluginValue` 与 `undo`。
3. **多选区恢复不完整。** 隔离上述撤销命令问题后，两个选区 `[0,2]`、`[3,5]` 的替换和文本撤销正常，但只恢复主选区 `[3,5]`。绑定只记录 `selection.main`，并通过 `EditorSelection.single` 恢复。
4. **其他本地写入不会刷新 CM6。** 同一 LoroDoc 上的本地非 CM6 插入会被同步监听的 `e.by === "local"` 条件忽略。我们需要的 Agent/文件系统统一事务入口必须处理这个差异；这是适配范围限制，不代表 CRDT 丢失操作。
5. **IME 分组仍需设计。** 实验使用 1,000ms 合并间隔并隔离官方撤销命令问题，远端导入仍可能拆分一次组合输入的撤销历史。应根据产品需要定义输入分组与远端编辑的交互。

此外，`getCursor(position, side)` 的 side 不应直接当作编辑器插入亲和性：在实测边界上，side 为 -1、0、1 的 cursor 都跟随原字符移动到远端插入之后，side 信息本身仍被保留。

这些结果支持继续验证 Loro 文档内核，但官方 CM6 绑定尚不能直接满足本项目的编辑验收条件。接入层需要处理初始化、事务回调边界、全部选区及外部写入。完整历史可作为初期实验基线；历史裁剪需要另外处理旧副本和锚点。

## 重跑

在本目录执行：

```sh
pnpm install --frozen-lockfile
cargo build --locked
pnpm test
pnpm test:interop
pnpm build
pnpm test:browser
```

本机 Nix 工具链的 native 构建使用：

```sh
CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --locked
```

`CHROME_BIN` 可指定其他 Chrome/Chromium 可执行文件。浏览器测试启动独立无头实例和临时本地 HTTP 服务，结束时关闭；不连接用户已有浏览器会话。

复核 Wasm 补丁版：

```sh
PROBE_LATEST=1 node build.mjs
PROBE_LATEST=1 node browser.test.mjs
PROBE_LATEST=1 node shallow-cursor.mjs
```

两个最小 panic 复现都预期非零退出：

```sh
node shallow-cursor.mjs
./target/debug/notist-loro-validation stale-cursor
```

运行结果写入忽略的 `results/`：`core.json`、`interop.json`、`browser.json`、`browser-latest.json`，以及 `browser.png`、快照和重放计划。测试源与 lockfile 保留在实验目录中。

## 覆盖边界

没有验证真实 macOS 输入法候选窗口、Safari/Firefox、长文档性能、持久化崩溃恢复、网络协议或 Yrs 对照。IME 用例由 Chromium CDP 产生组合事件；最终 compositionend 可能由 CM6 合成，不能等同于真实操作系统输入法回归。浏览器使用 npm 包提供的 Wasm，尚未建立正式 Rust editor-core 与 CM6 的绑定。
