= Notist Crate Architecture

依赖方向：A → B 表示 A 依赖 B。

- **实线**：结构性依赖（引擎与基础设施）；
- **虚线**：概念上的插件装载——被指向的 package 当前以内置 native crate
  静态注册实现，但与普通插件共享同一 contribution 模型，backend 可替换
  （见 designs/plugin-system/runtime-composition.not）：
  - `notist-plugin-core`：语义侧标准 package，由 analysis（组合根）预装；
  - `notist-html`：投影侧 target package，由 service 消费。
- 第三方 Wasm 插件不在图中成节点：由 analysis 经 `notist-plugin-host`
  在运行时动态装载。

```mermaid
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
```
