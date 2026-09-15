= Code 模式

Markup 中 `#` 进入 Code，Code 中 `[...]` 创建 Content literal 并回到 Markup。本页演示 Code 模式的书写能力，每处表达式的求值结果都直接渲染在页面上。

== 绑定与函数

#let accent = "violet"
#let double = (x: Int) => x * 2
#let triple(x: Int) -> Int = x * 3

lambda 调用：#(double(21))；函数定义糖：#(triple(14))。

== 表达式嵌入

- 括号嵌入 `#(1 + 2 * 3)` → #(1 + 2 * 3)
- Code block `#{ let x = 40; x + 2 }` → #{ let x = 40; x + 2 }
- if 表达式 `#if 2 > 1 [为真] else [为假]` → #if 2 > 1 [为真] else [为假]
- 紧跟文字时用 `;` 结束嵌入：#(double(2));倍（`;` 被消费，不产生输出）。

== 运算符

比较与逻辑：#(1 < 2 and 2 < 3) / #(1 > 2 or 2 > 1) / #(not false)。优先级从高到低：一元 `-` `not`，`*` `/`，`+` `-`，比较与 `==` `!=`，`and`，`or`。

== String 四种形态

#let escaped = "转义形态：\n 换行"
#let multi = """
多行形态：opening 后立即换行，
保留每一行。
"""
#let raw = r#"原始形态：\n 不转义"#

#(escaped)#(multi)#(raw)

== 注释与 import

Code 上下文支持 `//` 行注释与 `/* ... */` 嵌套块注释：#{ /* 块注释 */ let y = 1; y + 1 }。

#import <super::submodule>::{shared}

从子模块导入绑定：#(shared)
