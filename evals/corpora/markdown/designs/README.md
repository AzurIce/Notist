= Designs

#[
  Notist 的设计文档。

  目前 Notist 的 crate 架构：

  ```
               notist-plugin-sdk ->
    notist-eval -> notist-syntax -> notist-model

    notist-service -> notist-analysis -> notist-plugin-host -> notist-eval
  ```

```not
  #mermaid(
    source: r#"""
    graph TD
        cli["notist-cli<br>binary：命令面 / LSP / preview / skill"]
        service["notist-service<br>daemon / transport / watcher / query"]
        analysis["notist-analysis<br>分析层：check / 快照 / Analyzer View<br>语义侧组合根"]
        htm["notist-html<br>HTML target package<br>投影 registry + serializer<br>（内置 native 实现）"]
        phost["notist-plugin-host<br>Wasm 组件宿主<br>运行时加载 vault 内第三方插件"]
        pcore["notist-plugin-core<br>core 标准语义 package<br>（内置 native 实现）"]
        psdk["notist-plugin-sdk<br>插件作者 SDK"]
        evl["notist-eval<br>求值引擎与规约"]
        syn["notist-syntax<br>mode-aware 解析与 AST"]
        model["notist-model<br>共享数据模型"]

        cli --> service
        service --> analysis
        analysis --> phost
        htm --> evl
        phost --> evl
        pcore --> evl
        evl --> syn
        syn --> model
        psdk --> model

        analysis -.->|"预装 core package"| pcore
        service -.->|"HTML target"| htm
    """#,
    )[]
```
  ]

```text
pipeline/      编译管线：parse → check → evaluate → structure → project
language/      语言层：与实现宿主无关的语义规范
world/         Vault 层：模块、引用与运行边界
host/          宿主层：CLI、LSP、daemon、分析与渲染架构
lsp/           LSP 对外契约的黑箱无头验证
plugin-system/ 插件系统：package、ABI、贡献、信任模型与生命周期
```

== 阅读顺序

=== 入口

1. `<vault::designs::pipeline>`：编译管线总览：parse → check → evaluate → structure → project。
2. [overview](overview.md)：语言概览与最小示例。

=== 管线阶段

按数据流阅读：

1. [parse](pipeline/parse.md)：解析阶段与 mode-aware 语法树。
2. [evaluate](pipeline/evaluate.md)：求值 + Call Reduction 与 call 森林。
3. [structure](pipeline/structure.md)：递归成型与结构化重写。
4. [project](pipeline/project.md)：按 name 分发到 target 投影。
5. [plugin-call-reduction](pipeline/plugin-call-reduction.md)：插件调用在 reduce 阶段如何规约。

=== 插件系统

按模块依赖阅读：

1. `<vault::designs::plugin-system>`：PluginSystem 模块入口与 pipeline / world / host 边界。
2. [package](plugin-system/package.md)：插件 package、manifest、core package 与生命周期。
3. [core](plugin-system/core.md)：core package 的内容词表——内置构造器签名与校验。
4. [abi](plugin-system/abi.md)：共享类型、WIT 边界与语义 ABI。
5. [eval-contribution](plugin-system/eval-contribution.md)：语义函数贡献与 FunctionRegistry。
6. [capability](plugin-system/capability.md)：插件信任模型与能力系统删除裁决。
7. [safety](plugin-system/safety.md)：终止性、资源预算与确定性。
8. [shaping](plugin-system/shaping.md)：成型 schema 贡献。
9. [projection](plugin-system/projection.md)：target 投影贡献与 fallback。

=== 语言层

按概念依赖顺序阅读：

1. [markup-surface](language/markup-surface.md)：Markup 表面语法与 scope 书写形态。
2. [code-grammar](language/code-grammar.md)：Code 核心语法。
3. [type-system](language/type-system.md)：核心类型系统。
4. [collection-types](language/collection-types.md)：集合类型草案，尚未进入 current surface（最小 union 切片见 [target-and-union](language/target-and-union.md)）。
5. [scope-environment](language/scope-environment.md)：词法作用域与值层 scope。
6. [call-model](language/call-model.md)：函数签名与调用模型；rest 部分待实现。
7. [property-table](language/property-table.md)：旁置属性表。
8. [annotation-syntax](language/annotation-syntax.md)：标注语法。
9. [syntax-sugar](language/syntax-sugar.md)：Markup 语法糖规格。

=== Vault 层

1. [boundary-discovery](world/boundary-discovery.md)：Vault 边界、marker 与发现规则。
2. [module-result](world/module-result.md)：Module 与 ModuleResult。
3. [import](world/import.md)：显式 import 与依赖图。
4. [reference-ref-target](world/reference-ref-target.md)：引用寻址与 RefTarget。
5. [core-namespace-plugin-boundary](world/core-namespace-plugin-boundary.md)：core namespace、prelude 与插件边界。

=== 宿主层

1. [analyzer-snapshot](host/analyzer-snapshot.md)：分析层与 WorkspaceSnapshot。
2. [query-contract](host/query-contract.md)：查询契约与 Selector/Citation。
3. [search-retrieval-index](host/search-retrieval-index.md)：Search 模式与检索索引。
4. [daemon-process-views](host/daemon-process-views.md)：daemon 进程与 Analyzer View。
5. [client-interface-protocol](host/client-interface-protocol.md)：client interface、本地协议与生命周期。
6. [cli-command-surface-skill-distribution](host/cli-command-surface-skill-distribution.md)：CLI 产品边界与官方 Skill 分发；完整逐命令规范和使用文档在 [cli](../cli.md)。
7. [lsp-adapter](host/lsp-adapter.md)：LSP 适配。
8. [test](lsp/test.md)：LSP 无头契约测试：黑箱层判据、harness 确定性规则与场景目录。
9. [html-renderer](host/html-renderer.md)：HTML 渲染。
10. [build-preview](host/build-preview.md)：静态构建与本地预览。

== 状态属性

每篇 active design 在文件开头用模块属性声明实现对齐状态：

```notist
@![implementation = "aligned"]
```

取值：

| 值 | 含义 |
| --- | --- |
| `aligned` | 设计与当前实现一致 |
| `partial` | 设计是现行方向，但实现尚未完全对齐 |
| `missing` | 设计尚未进入实现 |

当前已知的非 `aligned` 文档：


- [collection-types](language/collection-types.md)：`missing`
- [call-model](language/call-model.md)：`partial`（rest 调用模型未实现）
- `<vault::designs::pipeline>`：`aligned`（统一 Node 管线与 HTML target 二阶段投影已实现）
- [evaluate](pipeline/evaluate.md)：`partial`（Node 值域已对齐；递归绑定与部分语言细节仍待实现）
- `<vault::designs::plugin-system>`：`partial`（完整 native/Wasm App API 与独立 runtime crate 尚未抽出）
- [package](plugin-system/package.md)：`partial`（package 模型与 core/native contribution 已实现；完整 package lifecycle/API 尚未收口）
- [runtime-composition](plugin-system/runtime-composition.md)：`partial`（WorkspaceSnapshot 与统一 `PluginContribution` 已实现；独立 App/runtime crate 尚未抽出）
- [eval-contribution](plugin-system/eval-contribution.md)：`partial`（native/Wasm 已走统一 contribution；高层 native Plugin trait 尚未提供）
- [core-namespace-plugin-boundary](world/core-namespace-plugin-boundary.md)：`partial`（namespace policy 已明确；完整 package policy 校验仍需收口）
- [capability](plugin-system/capability.md)：`partial`（backend 信任边界已裁定；统一 App composition 尚未收口）
- [safety](plugin-system/safety.md)：`partial`（精确循环检测与完整 App 资源治理仍待收口）
- [plugin-call-reduction](pipeline/plugin-call-reduction.md)：`aligned`（统一 Node、handler 不动点与 target-side 投影边界已实现）
- [project](pipeline/project.md)：`aligned`（HTML target 已有独立 projection registry 与 serializer）
- [projection](plugin-system/projection.md)：`partial`（Node 投影规约与 fallback 已实现；trusted/词表收口仍待定）
- [query-contract](host/query-contract.md)：`partial`（text-only 投影、`read` citation footer 与 `--exclude-scope` 尚未完整实现）
- [search-retrieval-index](host/search-retrieval-index.md)：`partial`（`--exclude-scope` 未实现）
- [cli-command-surface-skill-distribution](host/cli-command-surface-skill-distribution.md)：`partial`（2026-08-15 裁决的命令面未落地：`next`/`locate`/`refs`/`definition` 与 text-only 投影待实现）
- [cli](../cli.md)：`partial`（完整 CLI 命令设计+使用规范；目标为 2026-08-15 裁决的命令面，当前 binary 仍为过渡 surface）
- [lsp-adapter](host/lsp-adapter.md)：`partial`（Document Symbol 尚未覆盖函数与变量）
- [test](lsp/test.md)：`partial`（首批 7 场景已覆盖；错误规范接线、exit 非零码、cancel、跨文件联动、didSave 与 daemon 变体待补）

位置本身表达治理状态：active 目录中的文档是现行设计；退休的文档直接移除。Git 是完整历史，不维护编号 ID 或 `Design:` trailer。

== 写作规范

- 一篇文档回答一个概念或一组强耦合概念；目录层级只到 pipeline/language/world/host/lsp/plugin-system 为止，避免更深嵌套。
- 模块路径一旦发布，不因标题或阅读顺序调整而重命名；拆分或合并时才允许移动，并同步更新全部引用。
- 使用语义文件名；同名退休设计的区分交给 Git 历史。
- 设计正文解释模型、边界与后果；实现状态只在文件头属性中声明，不写进行文。
- 保持正文干练；判定规则用表，状态迁移用伪代码；每篇可独立 `notist check` 验证。

== 历史编号

本次重构前，设计文档使用 `D0001`–`D0036` 扁平编号。旧编号与语义路径的映射已随 archive 目录移除；旧提交仍可通过 Git 查看，不需要在当前树中保留编号壳。
