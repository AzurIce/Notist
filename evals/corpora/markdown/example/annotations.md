---
tags: reference
kind: [demo, reference]
status: current
---

<a id="scope-与标注"></a>
# Scope 与标注

标注为任意值绑定元数据：`@id` 赋 scope id、`#tag` 标签、`.class` 类名、`key = value` 键值属性。在 preview 中开启 Enhanced 模式，本页所有标注区域与属性都会高亮并列入右侧 Symbols 标签页。

<a id="scope-id-与锚点引用"></a>
## Scope id 与锚点引用

```not
#heading(level: 3)[显式 id 的标题]@explicit-id
```

同模块回链 [self](annotations.md#self)；跨模块 [annotations](annotations.md#explicit-id)。标题文本本身也是默认 id，例如 [annotations](annotations.md#标注属性)。

<a id="标注属性"></a>
## 标注属性

<!-- notist-block-annotation: #draft, #reviewed, .highlight, .wide, priority = 1, title = "块级标注", done = false -->
本段落携带两个 tag、两个 class，以及 String / Int / Bool 三种键值属性。

行内 postfix：#[被标注的内容]@inline-id,#inline,.chip,level=2 绑定到手动 scope `#[...]`，与后随普通文字混排。

模块属性 `@![...]` 写在文件开头（见本页源码首行），是模块元数据，不渲染到页面。
