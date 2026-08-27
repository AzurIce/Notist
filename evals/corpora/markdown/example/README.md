---
tags: example
status: stable
---

<a id="notist-语法示例"></a>
# Notist 语法示例

本页简明演示 Notist 的全部基础语法与功能，可直接用 `notist preview` 预览；精确规则见 [cheatsheet](../cheatsheet.md) 与 [grammar](../grammar.md)。

<a id="标题与行内样式"></a>
## 标题与行内样式

`=` / `==` 是标题糖。行内支持 **粗体**、_斜体_、***下划线***、~~删除线~~、`行内代码`；裸 [ 与 ] 按文本处理，转义用 \#、\[、\]、\@、\\。

<a id="列表与表格"></a>
## 列表与表格

- 无序条目 A
- 无序条目 B

1. 有序条目一
2. 有序条目二

| 名称 | 数值 |
| - | - |
| one | 1 |
| two | 2 |

<a id="分隔线与代码块"></a>
## 分隔线与代码块

---

```rust
fn main() {
    println!("hello notist");
}
```

<a id="显式调用"></a>
## 显式调用




```not
#heading(level: 3)[调用形式标题]
#callout(kind: "warning", title: [注意])[callout 正文内容。]
#details(summary: [点击展开])[details 正文内容。]
#figure(kind: "table", supplement: [表], caption: [内置表格])[
#table(columns: 2, header: true)[
#table-cell[名称] #table-cell[数值]
#table-cell[answer] #table-cell[42]
]
]
```

<a id="code-与标注"></a>
## Code 与标注

- `<code>`：Code 模式——`#let` 绑定、lambda、运算符、String 四种形态、注释与 `#import`；
- `<annotations>`：Scope 与标注——`@id` 锚点、`#tag`、`.class`、键值属性与模块属性。

<a id="引用"></a>
## 引用

`<submodule>` 相对引用；`<self::submodule/target>` 带锚点；[cheatsheet](../cheatsheet.md) 绝对路径。
