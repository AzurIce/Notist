//! 最小前端：按 grammar 的产生式直接解析源文本。
//!
//! 本切片覆盖 Section、行内流、空行、强调族与调用；不依赖旧 parser。
//! 这里只做识别与结构，不消解糖：脱糖与名字解析在 `crate::plan`。

use notist_model::TextRange;

use crate::ir::Diagnostic;

/// 一个源文件的前端产物。
pub struct Surface {
    pub pieces: Vec<Piece>,
    pub diagnostics: Vec<Diagnostic>,
}

/// 一个片段：块级构造、行内流，或空行。
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Section {
        level: usize,
        label: Vec<Inline>,
        body: Vec<Piece>,
        range: TextRange,
    },
    Run(Vec<Inline>),
    Parbreak {
        range: TextRange,
    },
}

/// 行内构造。
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text { value: String, range: TextRange },
    Strong { body: Vec<Inline>, range: TextRange },
    Emph { body: Vec<Inline>, range: TextRange },
    Underline { body: Vec<Inline>, range: TextRange },
    Strike { body: Vec<Inline>, range: TextRange },
    Call(Call),
}

/// 一次调用：名字、具名实参、可选尾随体。
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub args: Vec<(String, Literal)>,
    pub body: Option<Vec<Piece>>,
    pub range: TextRange,
}

/// 本切片的实参只有字面量；表达式语言在后续切片接入。
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    String(String),
}

/// 解析一个源文件。
pub fn parse(source: &str) -> Surface {
    let mut diagnostics = Vec::new();
    let lines = split_lines(source, 0, source.len());
    let pieces = parse_pieces(source, &lines, 0, lines.len(), &mut diagnostics);
    Surface {
        pieces,
        diagnostics,
    }
}

struct Line {
    start: usize,
    /// 行内容末尾，不含换行符。
    content_end: usize,
}

fn split_lines(source: &str, base: usize, end: usize) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = base;
    while start < end {
        let (content_end, next) = match source[start..end].find('\n') {
            Some(offset) => {
                let newline = start + offset;
                let mut content_end = newline;
                if content_end > start && source.as_bytes()[content_end - 1] == b'\r' {
                    content_end -= 1;
                }
                (content_end, newline + 1)
            }
            None => (end, end),
        };
        lines.push(Line { start, content_end });
        start = next;
    }
    lines
}

/// 空行只由 U+0020 构成；含 tab 的行按文本处理（L5）。
fn is_blank(source: &str, line: &Line) -> bool {
    source[line.start..line.content_end]
        .bytes()
        .all(|byte| byte == b' ')
}

/// 行首等号定界：返回层级与标题行内容的起始偏移。
fn section_head(source: &str, line: &Line) -> Option<(usize, usize)> {
    let text = &source[line.start..line.content_end];
    let margin = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[margin..];
    let level = rest.bytes().take_while(|byte| *byte == b'=').count();
    if level == 0 || !rest[level..].starts_with(' ') {
        return None;
    }
    Some((level, line.start + margin + level + 1))
}

fn parse_pieces(
    source: &str,
    lines: &[Line],
    start: usize,
    end: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut index = start;
    while index < end {
        if is_blank(source, &lines[index]) {
            pieces.push(Piece::Parbreak {
                range: TextRange::new(lines[index].start, lines[index].content_end),
            });
            index += 1;
            continue;
        }
        if let Some((level, label_start)) = section_head(source, &lines[index]) {
            let mut body_end = index + 1;
            while body_end < end {
                match section_head(source, &lines[body_end]) {
                    Some((nested, _)) if nested <= level => break,
                    _ => body_end += 1,
                }
            }
            let label = parse_inlines(source, label_start, lines[index].content_end, diagnostics);
            let body = parse_pieces(source, lines, index + 1, body_end, diagnostics);
            let range_end = if body_end > index + 1 {
                lines[body_end - 1].content_end
            } else {
                lines[index].content_end
            };
            pieces.push(Piece::Section {
                level,
                label,
                body,
                range: TextRange::new(lines[index].start, range_end),
            });
            index = body_end;
            continue;
        }
        let mut run_end = index;
        while run_end < end
            && !is_blank(source, &lines[run_end])
            && section_head(source, &lines[run_end]).is_none()
        {
            run_end += 1;
        }
        // 流段的范围跨行：行间的换行留在 text 的值里（L3）。
        let inlines = parse_inlines(
            source,
            lines[index].start,
            lines[run_end - 1].content_end,
            diagnostics,
        );
        pieces.push(Piece::Run(inlines));
        index = run_end;
    }
    pieces
}

fn parse_inlines(
    source: &str,
    start: usize,
    end: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Inline> {
    let mut inlines = Vec::new();
    let mut text_start = start;
    let mut cursor = start;
    while cursor < end {
        let Some(character) = source[cursor..end].chars().next() else {
            break;
        };
        let consumed = match character {
            '#' => match parse_call(source, cursor, end, diagnostics) {
                Some((call, next)) => {
                    push_text(source, &mut inlines, text_start, cursor);
                    inlines.push(Inline::Call(call));
                    text_start = next;
                    next - cursor
                }
                None => character.len_utf8(),
            },
            '*' | '_' | '~' => match parse_emphasis(source, cursor, end, diagnostics) {
                Some((inline, next)) => {
                    push_text(source, &mut inlines, text_start, cursor);
                    inlines.push(inline);
                    text_start = next;
                    next - cursor
                }
                None => character.len_utf8(),
            },
            _ => character.len_utf8(),
        };
        cursor += consumed;
    }
    push_text(source, &mut inlines, text_start, end);
    inlines
}

fn push_text(source: &str, inlines: &mut Vec<Inline>, start: usize, end: usize) {
    if start < end {
        inlines.push(Inline::Text {
            value: source[start..end].to_owned(),
            range: TextRange::new(start, end),
        });
    }
}

/// 强调族：定界符取最长匹配，开闭两侧都紧邻非空白内容才成立。
fn parse_emphasis(
    source: &str,
    at: usize,
    end: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Inline, usize)> {
    let rest = &source[at..end];
    let marker = if rest.starts_with("__") {
        "__"
    } else if rest.starts_with("~~") {
        "~~"
    } else if rest.starts_with('*') {
        "*"
    } else if rest.starts_with('_') {
        "_"
    } else {
        return None;
    };
    let inner_start = at + marker.len();
    let first = source[inner_start..end].chars().next()?;
    if first.is_whitespace() {
        return None;
    }
    let mut search = inner_start;
    while let Some(offset) = source[search..end].find(marker) {
        let close = search + offset;
        if close > inner_start
            && let Some(before) = source[inner_start..close].chars().next_back()
            && !before.is_whitespace()
        {
            let body = parse_inlines(source, inner_start, close, diagnostics);
            let range = TextRange::new(at, close + marker.len());
            let inline = match marker {
                "__" => Inline::Underline { body, range },
                "~~" => Inline::Strike { body, range },
                "*" => Inline::Strong { body, range },
                _ => Inline::Emph { body, range },
            };
            return Some((inline, close + marker.len()));
        }
        search = close + marker.len();
    }
    None
}

fn parse_call(
    source: &str,
    at: usize,
    end: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Call, usize)> {
    let mut cursor = at + 1;
    let name_start = cursor;
    while cursor < end {
        let character = source[cursor..end].chars().next()?;
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            cursor += 1;
        } else {
            break;
        }
    }
    if cursor == name_start {
        return None;
    }
    let name = source[name_start..cursor].to_owned();
    let mut args = Vec::new();
    if source[cursor..end].starts_with('(') {
        let (parsed, next) = parse_arguments(source, cursor, end, diagnostics);
        args = parsed;
        cursor = next;
    }
    let mut body = None;
    if source[cursor..end].starts_with('[') {
        match match_body(source, cursor, end) {
            Some((inner_start, inner_end, next)) => {
                let lines = split_lines(source, inner_start, inner_end);
                body = Some(parse_pieces(source, &lines, 0, lines.len(), diagnostics));
                cursor = next;
            }
            None => {
                diagnostics.push(Diagnostic::warn(
                    "unclosed-content-block",
                    "unclosed content block",
                    TextRange::new(at, end),
                ));
                cursor = end;
            }
        }
    }
    Some((
        Call {
            name,
            args,
            body,
            range: TextRange::new(at, cursor),
        },
        cursor,
    ))
}

/// 返回括号体的内容区间与闭合之后的位置。
fn match_body(source: &str, at: usize, end: usize) -> Option<(usize, usize, usize)> {
    let mut depth = 0usize;
    let mut cursor = at;
    while cursor < end {
        let character = source[cursor..end].chars().next()?;
        match character {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((at + 1, cursor, cursor + 1));
                }
            }
            _ => {}
        }
        cursor += character.len_utf8();
    }
    None
}

fn parse_arguments(
    source: &str,
    at: usize,
    end: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<(String, Literal)>, usize) {
    let mut args = Vec::new();
    let mut cursor = at + 1;
    loop {
        skip_spaces(source, &mut cursor, end);
        if source[cursor..end].starts_with(')') {
            return (args, cursor + 1);
        }
        if cursor >= end {
            diagnostics.push(Diagnostic::warn(
                "unclosed-argument-list",
                "unclosed argument list",
                TextRange::new(at, end),
            ));
            return (args, end);
        }
        let key_start = cursor;
        while cursor < end {
            let character = source[cursor..end].chars().next().expect("cursor < end");
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                cursor += 1;
            } else {
                break;
            }
        }
        if cursor == key_start {
            diagnostics.push(Diagnostic::warn(
                "invalid-argument",
                "expected an argument name",
                TextRange::new(cursor, end),
            ));
            return (args, end);
        }
        let key = source[key_start..cursor].to_owned();
        skip_spaces(source, &mut cursor, end);
        if !source[cursor..end].starts_with(':') {
            diagnostics.push(Diagnostic::warn(
                "invalid-argument",
                format!("argument `{key}` is missing `:`"),
                TextRange::new(key_start, end),
            ));
            return (args, end);
        }
        cursor += 1;
        skip_spaces(source, &mut cursor, end);
        match parse_literal(source, cursor, end) {
            Some((literal, next)) => {
                args.push((key, literal));
                cursor = next;
            }
            None => {
                diagnostics.push(Diagnostic::warn(
                    "invalid-argument",
                    format!("argument `{key}` expects a literal value"),
                    TextRange::new(key_start, end),
                ));
                return (args, end);
            }
        }
        skip_spaces(source, &mut cursor, end);
        if source[cursor..end].starts_with(',') {
            cursor += 1;
            continue;
        }
        if source[cursor..end].starts_with(')') {
            return (args, cursor + 1);
        }
        diagnostics.push(Diagnostic::warn(
            "invalid-argument",
            "expected `,` or `)`",
            TextRange::new(cursor, end),
        ));
        return (args, end);
    }
}

fn parse_literal(source: &str, at: usize, end: usize) -> Option<(Literal, usize)> {
    let rest = &source[at..end];
    if let Some(after) = rest.strip_prefix('"') {
        let mut value = String::new();
        let mut characters = after.char_indices();
        while let Some((offset, character)) = characters.next() {
            match character {
                '"' => return Some((Literal::String(value), at + 1 + offset + 1)),
                '\\' => {
                    let (_, escaped) = characters.next()?;
                    value.push(escaped);
                }
                _ => value.push(character),
            }
        }
        return None;
    }
    if rest.starts_with("true") {
        return Some((Literal::Bool(true), at + 4));
    }
    if rest.starts_with("false") {
        return Some((Literal::Bool(false), at + 5));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let value = rest[..digits].parse().ok()?;
        return Some((Literal::Int(value), at + digits));
    }
    None
}

fn skip_spaces(source: &str, cursor: &mut usize, end: usize) {
    while *cursor < end && source.as_bytes()[*cursor] == b' ' {
        *cursor += 1;
    }
}

// ---------------------------------------------------------------------------
// 声明模块：只有 extern 声明，没有内容。
// ---------------------------------------------------------------------------

/// 一条 extern 声明的头部种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclHead {
    /// 函数：不产内容。
    Function,
    /// 元素：独立流。
    Element,
    /// 元素：行内流。
    Inline,
}

impl DeclHead {
    pub const fn head_word(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Element | Self::Inline => "element",
        }
    }
}

/// 一个声明的形参。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclParam {
    pub name: String,
    pub ty: String,
}

/// 声明模块里的一条 extern 声明。
#[derive(Clone, Debug, PartialEq)]
pub struct ExternDecl {
    pub head: DeclHead,
    pub name: String,
    pub params: Vec<DeclParam>,
    pub result: String,
    pub range: TextRange,
}

/// 解析一个声明模块：只接受 `#extern` 行，其他内容报 warn。
pub fn parse_declarations(source: &str) -> (Vec<ExternDecl>, Vec<Diagnostic>) {
    let mut declarations = Vec::new();
    let mut diagnostics = Vec::new();
    let lines = split_lines(source, 0, source.len());
    for line in &lines {
        if is_blank(source, line) {
            continue;
        }
        let text = &source[line.start..line.content_end];
        let margin = text.len() - text.trim_start_matches(' ').len();
        let body = &text[margin..];
        let range = TextRange::new(line.start, line.content_end);
        let Some(rest) = body.strip_prefix("#extern") else {
            diagnostics.push(Diagnostic::warn(
                "declaration-module-has-content",
                "declaration modules contain only `#extern` declarations",
                range,
            ));
            continue;
        };
        match parse_declaration(rest, range) {
            Ok(declaration) => declarations.push(declaration),
            Err(message) => {
                diagnostics.push(Diagnostic::warn("invalid-declaration", message, range));
            }
        }
    }
    (declarations, diagnostics)
}

fn parse_declaration(text: &str, range: TextRange) -> Result<ExternDecl, String> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    let head_word = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default().trim();
    let head = match head_word {
        "fn" => DeclHead::Function,
        "element" => DeclHead::Element,
        "inline" => DeclHead::Inline,
        other => return Err(format!("expected `fn`, `element` or `inline`, found `{other}`")),
    };
    let name_end = rest
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '-')
        })
        .unwrap_or(rest.len());
    if name_end == 0 {
        return Err("expected a declaration name".to_owned());
    }
    let name = rest[..name_end].to_owned();
    let rest = rest[name_end..].trim_start();
    let Some(rest) = rest.strip_prefix('(') else {
        return Err(format!("declaration `{name}` expects `(`"));
    };
    let Some(close) = rest.find(')') else {
        return Err(format!("declaration `{name}` is missing `)`"));
    };
    let params_text = &rest[..close];
    let rest = rest[close + 1..].trim_start();
    let Some(rest) = rest.strip_prefix("->") else {
        return Err(format!("declaration `{name}` expects `->`"));
    };
    let result = rest.trim().to_owned();
    if result.is_empty() {
        return Err(format!("declaration `{name}` is missing a result type"));
    }
    let mut params = Vec::new();
    for part in params_text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((param, ty)) = part.split_once(':') else {
            return Err(format!("parameter `{part}` expects `name: Type`"));
        };
        params.push(DeclParam {
            name: param.trim().to_owned(),
            ty: ty.trim().to_owned(),
        });
    }
    Ok(ExternDecl {
        head,
        name,
        params,
        result,
        range,
    })
}
