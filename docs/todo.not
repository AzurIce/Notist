== 存在的问题

- #[下面的内容删除线没有正确在 preview 中显示出来]@#parse
  ```
  - ~~`notist.wasm` 兼容性~~（已实锤并解决）
  ```

== 已解决

- #[下面两段解析结果不一致，换行会导致 preview 里两段之间多一个空格]@#parse @date="2026-08-24"
  ```notist
  既然我们无法扩展 Obsidian 自己的双链、图谱等系统，那么不如干脆与 Obsidian 自己的系统完全隔离。目前的想法是，我们将一个 Obsidian Vault 分为两个 World，一个是它本身原生的那套 Markdown World，一个是我们的 Notist World。
  ```

  ```notist
  既然我们无法扩展 Obsidian 自己的双链、图谱等系统，那么不如干脆与 Obsidian 自己的系统完全隔离。
  目前的想法是，我们将一个 Obsidian Vault 分为两个 World，一个是它本身原生的那套 Markdown World，一个是我们的 Notist World。
  ```

- #[在 `[[]]` 里输入 `#` 也会触发函数符号的补全]@#lsp @date="2026-08-24"

- #[List 内无法使用换行的 `#[]` scope]@#parse @date="2026-08-24"
  ```notist
  - #[
    ...
  ]
  ```
- #[代码块没法在缩进到列表内]@#parse @date="2026-08-24"
