"#" @punctuation.special

"[" @punctuation.bracket
"]" @punctuation.bracket
"(" @punctuation.bracket
")" @punctuation.bracket
"{" @punctuation.bracket
"}" @punctuation.bracket
"," @punctuation.delimiter
";" @punctuation.delimiter
":" @punctuation.delimiter
"=" @operator
"::" @punctuation.delimiter
"@" @attribute
"?" @operator
"=>" @operator
"->" @operator

["let" "if" "else" "fn" "trailing"] @keyword
["import" "as"] @keyword.import
"not" @keyword.operator

[(or_operator)
 (and_operator)] @keyword.operator

[(comparison_operator)
 (additive_operator)
 (multiplicative_operator)] @operator

(rule_marker) @punctuation.special

(table_delimiter_row) @punctuation.special
(pipe) @punctuation.delimiter

[(block_attributes_open)
 (module_attributes_open)] @attribute

(let_expression
  parameters: (parameters)
  name: (identifier) @function)

(parameter
  name: (identifier) @variable.parameter)

(type_expression
  (type_member
    (qualified_name) @type))

(target_literal
  target: (_) @module
  ">" @punctuation.bracket)

(target_open) @punctuation.bracket

(import_item
  alias: (identifier) @variable)

(heading
  marker: (heading_marker) @punctuation.special
  body: (inline_body) @title.markup)

[(list_item marker: (list_marker) @punctuation.list_marker.markup)
 (enum_item marker: (enum_marker) @punctuation.list_marker.markup)
 (task_item marker: (task_marker) @punctuation.list_marker.markup)]

[(strong_marker)
 (emphasis_marker)
 (underline_marker)
 (strike_marker)] @punctuation.special

(strong) @emphasis.strong
(emphasis) @emphasis
(underline) @emphasis
(strike) @emphasis.strikethrough
(escaped_punctuation) @string.escape

[(line_comment)
 (block_comment)] @comment

(call_expression
  function: (qualified_name) @function.call)

(named_argument
  name: (identifier) @variable.parameter)

(none) @constant.builtin
(boolean) @boolean
(integer) @number
(float) @number
(string) @string
(escape_sequence) @string.escape

[(escaped_inline_open)
 (escaped_multiline_open)
 (raw_inline_open)
 (raw_multiline_open)
 (string_close)] @punctuation.bracket

(inline_raw) @string.special
(fence_open) @punctuation.bracket
(fence_info) @label
(fence_content) @string.special
(fence_close) @punctuation.bracket

(id_attribute
  name: (identifier) @label)

(tag_attribute
  name: (identifier) @tag)

(class_attribute
  name: (identifier) @type)

(property_attribute
  key: (identifier) @property)

(attribute_string) @string
