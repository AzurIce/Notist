use super::{Expr, ExprKind, Parser};

fn expr(kind: ExprKind, start: usize, end: usize) -> Expr {
    Expr {
        kind,
        offset: start,
        end,
        id: 0,
    }
}
fn string(value: impl Into<String>, start: usize, end: usize) -> Expr {
    expr(ExprKind::String(value.into()), start, end)
}
fn element(name: &str, fields: Vec<(String, Expr)>, start: usize, end: usize) -> Expr {
    expr(ExprKind::Element(name.into(), fields), start, end)
}
fn body(name: &str, parts: Vec<Expr>, start: usize, end: usize) -> Expr {
    element(
        name,
        vec![("body".into(), expr(ExprKind::Content(parts), start, end))],
        start,
        end,
    )
}
fn flush(out: &mut Vec<Expr>, inline: &mut Vec<Expr>, wrap: bool) {
    while inline
        .last()
        .is_some_and(|e| matches!(&e.kind, ExprKind::String(s) if s == " "))
    {
        inline.pop();
    }
    if inline.is_empty() {
        return;
    }
    let start = inline[0].offset;
    let end = inline.last().unwrap().end;
    let parts = std::mem::take(inline);
    if parts.len() == 1 && matches!(parts[0].kind, ExprKind::Call(..)) {
        out.extend(parts);
        return;
    }
    if wrap {
        out.push(body("paragraph", parts, start, end));
    } else {
        out.extend(parts);
    }
}

fn word(c: char) -> bool {
    c.is_alphanumeric()
        && !(('\u{2e80}'..='\u{9fff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c))
}

impl Parser<'_> {
    fn line_position(&self) -> Option<usize> {
        let line = self.source[..self.pos].rsplit('\n').next().unwrap_or("");
        line.chars()
            .all(|c| c == ' ' || c == '\t')
            .then(|| line.chars().count())
    }
    fn rest(&self) -> &str {
        &self.source[self.pos..]
    }
    fn consume_char(&mut self) -> Option<char> {
        let c = self.rest().chars().next()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn marker(&self) -> Option<(String, usize, Option<i64>)> {
        self.line_position()?;
        let rest = self.rest();
        let (name, length, number) = if rest.starts_with("- ") || rest.starts_with("-\t") {
            ("list", 1, None)
        } else if rest.starts_with("+ ") || rest.starts_with("+\t") {
            ("enum", 1, None)
        } else if rest.starts_with("/ ") || rest.starts_with("/\t") {
            ("terms", 1, None)
        } else {
            let length = rest.bytes().take_while(u8::is_ascii_digit).count();
            if length == 0 || !rest[length..].starts_with(". ") {
                return None;
            }
            ("enum", length + 1, Some(rest[..length].parse().ok()?))
        };
        Some((name.into(), length, number))
    }
    pub(super) fn markup_flow(
        &mut self,
        end: Option<char>,
        section: usize,
        indent: Option<usize>,
        mut wrap: bool,
    ) -> Result<Vec<Expr>, String> {
        if self.depth >= 128 {
            return Err("markup nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = self.flow_inner(end, section, indent, &mut wrap);
        self.depth -= 1;
        result
    }
    fn flow_inner(
        &mut self,
        end: Option<char>,
        section: usize,
        indent: Option<usize>,
        wrap: &mut bool,
    ) -> Result<Vec<Expr>, String> {
        let mut out = Vec::new();
        let mut inline = Vec::new();
        let mut brackets = 0usize;
        loop {
            let start = self.pos;
            let Some(c) = self.rest().chars().next() else {
                if end.is_some() && section == 0 && indent.is_none() {
                    return Err("unclosed markup".into());
                }
                break;
            };
            if self.line_position().is_some() {
                while self.rest().starts_with([' ', '\t']) {
                    self.consume_char();
                }
                if let Some(min) = indent
                    && !self.rest().starts_with(['\n', '\r'])
                    && self.line_position().is_some_and(|n| n <= min)
                {
                    break;
                }
            }
            if self.pos != start {
                continue;
            }
            if end == Some(c) && brackets == 0 {
                if section == 0 && indent.is_none() {
                    self.consume_char();
                }
                break;
            }
            let level = self.rest().bytes().take_while(|c| *c == b'=').count();
            if self.line_position().is_some()
                && level > 0
                && self.rest()[level..].starts_with([' ', '\t', '\n'])
            {
                if section > 0 && level <= section {
                    break;
                }
                flush(&mut out, &mut inline, *wrap);
                self.pos += level;
                while self.rest().starts_with([' ', '\t']) {
                    self.consume_char();
                }
                let title = self.markup_inline('\n', true)?;
                let children = self.markup_flow(end, level, indent, true)?;
                out.push(expr(
                    ExprKind::Section(level, title, children),
                    start,
                    self.pos,
                ));
                continue;
            }
            if let Some((kind, _, _)) = self.marker() {
                flush(&mut out, &mut inline, *wrap);
                out.push(self.markup_list(kind, end)?);
                continue;
            }
            if c.is_whitespace() {
                let mut newlines = 0;
                while let Some(c) = self.rest().chars().next().filter(|c| c.is_whitespace()) {
                    if c == '\n' {
                        newlines += 1;
                    }
                    self.consume_char();
                }
                if newlines >= 2 {
                    *wrap = true;
                    flush(&mut out, &mut inline, true);
                } else if !inline.is_empty()
                    && !inline
                        .last()
                        .is_some_and(|e| matches!(&e.kind, ExprKind::String(s) if s == " "))
                {
                    inline.push(string(" ", start, self.pos));
                }
                continue;
            }
            if self.rest().starts_with("//") || self.rest().starts_with("/*") {
                self.markup_comment()?;
                continue;
            }
            if c == '@' && !self.documentation {
                if self.line_position().is_some()
                    && let Some(level) = self.annotated_heading()
                {
                    // Leading Item annotations belong to the following heading,
                    // including when that heading closes the current section.
                    if section > 0 && level <= section {
                        break;
                    }
                    flush(&mut out, &mut inline, *wrap);
                }
                let annotation = self.markup_annotation()?;
                if inline.is_empty() {
                    out.push(annotation);
                    *wrap = true;
                } else {
                    inline.push(annotation);
                }
                continue;
            }
            if c == '#' && !self.documentation {
                let value = self.markup_code()?;
                if matches!(value.kind, ExprKind::Declaration(_)) {
                    flush(&mut out, &mut inline, *wrap);
                    out.push(value);
                } else {
                    inline.push(value);
                }
                continue;
            }
            if c == '[' && !self.rest().starts_with("[[") {
                brackets += 1;
            }
            if c == ']' {
                if brackets == 0 {
                    return Err("unmatched `]` in markup; escape it with a backslash".into());
                }
                brackets -= 1;
            }
            let value = self.markup_atom()?;
            let block = matches!(&value.kind, ExprKind::Element(name, fields) if (name == "raw" || name == "math") && fields.iter().any(|(key,v)| key == "block" && matches!(v.kind, ExprKind::Bool(true))));
            if block {
                flush(&mut out, &mut inline, *wrap);
                out.push(value);
            } else {
                inline.push(value);
            }
        }
        flush(&mut out, &mut inline, *wrap);
        Ok(out)
    }
    fn markup_inline(&mut self, end: char, allow_eof: bool) -> Result<Vec<Expr>, String> {
        if self.depth >= 128 {
            return Err("markup nesting limit exceeded".into());
        }
        self.depth += 1;
        let result = (|| {
            let mut parts = Vec::new();
            while let Some(c) = self.rest().chars().next() {
                let inside_word = matches!(c, '*' | '_')
                    && self.source[..self.pos]
                        .chars()
                        .next_back()
                        .is_some_and(word)
                    && self.rest()[1..].chars().next().is_some_and(word);
                if c == end && !inside_word {
                    self.consume_char();
                    return Ok(parts);
                }
                if c == '\n' && end == ':' {
                    return Err("term requires `:` on the same line".into());
                }
                if c == '\n'
                    && end != '\n'
                    && self
                        .rest()
                        .trim_start_matches([' ', '\t', '\r', '\n'])
                        .len()
                        + 1
                        < self.rest().len()
                {
                    let whitespace = self
                        .rest()
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect::<String>();
                    if whitespace.matches('\n').count() >= 2 {
                        return Err("inline markup cannot cross a paragraph break".into());
                    }
                }
                if self.rest().starts_with("//") || self.rest().starts_with("/*") {
                    self.markup_comment()?;
                    continue;
                }
                let start = self.pos;
                if c.is_whitespace() {
                    self.consume_char();
                    if !parts.is_empty()
                        && !parts.last().is_some_and(
                            |e: &Expr| matches!(&e.kind, ExprKind::String(s) if s == " "),
                        )
                    {
                        parts.push(string(" ", start, self.pos));
                    }
                } else if c == '@' && !self.documentation {
                    parts.push(self.markup_annotation()?);
                } else if c == '#' && !self.documentation {
                    parts.push(self.markup_code()?);
                } else {
                    parts.push(self.markup_atom()?);
                }
            }
            if allow_eof {
                Ok(parts)
            } else {
                Err(format!("unclosed markup, expected `{end}`"))
            }
        })();
        self.depth -= 1;
        result
    }
    fn markup_code(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.pos += 1;
        self.record(start, self.pos, "markup-interp", "#");
        let value = if self.at("let") || self.at("use") || self.at("wasm") {
            let statement = self.statement()?;
            expr(ExprKind::Declaration(Box::new(statement)), start, self.pos)
        } else {
            self.expr(5)?
        };
        if self.rest().starts_with(';') {
            self.pos += 1;
        }
        Ok(value)
    }
    fn markup_annotation(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        self.pos += 1;
        let module = self.rest().starts_with('!');
        if module {
            self.pos += 1;
        }
        self.record(start, self.pos, "annotation", &self.source[start..self.pos]);
        let value = self.expr(5)?;
        Ok(expr(
            ExprKind::Annotation(module, Box::new(value)),
            start,
            self.pos,
        ))
    }
    /// Look past a standalone run of Item annotations without consuming its tokens.
    /// Module annotations and other expressions keep their existing lexical scope.
    fn annotated_heading(&mut self) -> Option<usize> {
        let position = self.pos;
        let tokens = self.tokens.len();
        let level = (|| {
            loop {
                if !self.rest().starts_with('@') || self.rest().starts_with("@!") {
                    return None;
                }
                self.markup_annotation().ok()?;
                loop {
                    while self.rest().starts_with(char::is_whitespace) {
                        self.consume_char();
                    }
                    if self.rest().starts_with("//") || self.rest().starts_with("/*") {
                        self.markup_comment().ok()?;
                    } else {
                        break;
                    }
                }
                self.line_position()?;
                let level = self.rest().bytes().take_while(|c| *c == b'=').count();
                if level > 0 && self.rest()[level..].starts_with([' ', '\t', '\n']) {
                    return Some(level);
                }
            }
        })();
        self.pos = position;
        self.tokens.truncate(tokens);
        level
    }
    fn markup_comment(&mut self) -> Result<(), String> {
        let start = self.pos;
        if self.rest().starts_with("//") {
            self.pos += self.rest().find('\n').unwrap_or(self.rest().len());
        } else {
            self.pos += 2;
            let mut nesting = 1;
            while nesting > 0 {
                if self.rest().starts_with("/*") {
                    self.pos += 2;
                    nesting += 1;
                } else if self.rest().starts_with("*/") {
                    self.pos += 2;
                    nesting -= 1;
                } else if self.consume_char().is_none() {
                    return Err("unclosed block comment".into());
                }
            }
        }
        self.record(start, self.pos, "comment", &self.source[start..self.pos]);
        Ok(())
    }
    fn markup_atom(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let c = self.rest().chars().next().ok_or("expected markup")?;
        if c == '`' {
            return self.markup_raw();
        }
        if c == '$' {
            self.pos += 1;
            let begin = self.pos;
            let mut escaped = false;
            let mut quoted = false;
            while let Some(c) = self.consume_char() {
                if c == '"' && !escaped {
                    quoted = !quoted;
                }
                if c == '$' && !escaped && !quoted {
                    let text = &self.source[begin..self.pos - 1];
                    let block = text.starts_with(char::is_whitespace)
                        && text.ends_with(char::is_whitespace);
                    let value = element(
                        "math",
                        vec![
                            ("content".into(), string(text.trim(), begin, self.pos - 1)),
                            ("block".into(), expr(ExprKind::Bool(block), start, self.pos)),
                        ],
                        start,
                        self.pos,
                    );
                    self.record(start, self.pos, "math", &self.source[start..self.pos]);
                    return Ok(value);
                }
                escaped = c == '\\' && !escaped;
            }
            return Err("unclosed math".into());
        }
        if self.rest().starts_with("[[") {
            self.pos += 2;
            let tokens = self.tokens.len();
            let first = self.segment()?;
            let (module, labels) = self.reference_path(first)?;
            let close = self.token().offset;
            if !self.source[close..].starts_with("]]") {
                return Err("expected `]]` after wikilink target".into());
            }
            self.pos = close + 2;
            self.tokens.truncate(tokens);
            self.record(start, self.pos, "wikilink", &self.source[start..self.pos]);
            return Ok(expr(ExprKind::Target(module, labels), start, self.pos));
        }
        if self.rest().starts_with("https://") || self.rest().starts_with("http://") {
            let mut target = self
                .rest()
                .split_whitespace()
                .next()
                .unwrap()
                .trim_end_matches(['.', ',', ';', '!', '?'])
                .to_owned();
            while target.ends_with(')') && target.matches(')').count() > target.matches('(').count()
            {
                target.pop();
            }
            while target.ends_with(']') && target.matches(']').count() > target.matches('[').count()
            {
                target.pop();
            }
            self.pos += target.len();
            return Ok(element(
                "link",
                vec![
                    ("dest".into(), string(&target, start, self.pos)),
                    (
                        "body".into(),
                        expr(
                            ExprKind::Content(vec![string(target, start, self.pos)]),
                            start,
                            self.pos,
                        ),
                    ),
                ],
                start,
                self.pos,
            ));
        }
        if c == '\\' {
            self.pos += 1;
            if self.rest().is_empty() || self.rest().starts_with(char::is_whitespace) {
                return Ok(element("linebreak", vec![], start, self.pos));
            }
            if self.rest().starts_with("u{") {
                self.pos += 2;
                let end = self.rest().find('}').ok_or("unclosed Unicode escape")? + self.pos;
                let number = u32::from_str_radix(&self.source[self.pos..end], 16)
                    .map_err(|_| "invalid Unicode escape")?;
                let c = char::from_u32(number).ok_or("invalid Unicode code point")?;
                self.pos = end + 1;
                return Ok(string(c.to_string(), start, self.pos));
            }
            let next = self.consume_char().unwrap();
            return Ok(string(next.to_string(), start, self.pos));
        }
        if c == '*' || c == '_' {
            let inside = self.source[..start].chars().next_back().is_some_and(word)
                && self.rest()[1..].chars().next().is_some_and(word);
            if !inside {
                self.pos += 1;
                let parts = self.markup_inline(c, false)?;
                return Ok(expr(
                    ExprKind::Styled(if c == '*' { "strong" } else { "em" }, parts),
                    start,
                    self.pos,
                ));
            }
        }
        for (syntax, value) in [
            ("---", "\u{2014}"),
            ("--", "\u{2013}"),
            ("...", "\u{2026}"),
            ("-?", "\u{ad}"),
            ("~", "\u{a0}"),
        ] {
            if self.rest().starts_with(syntax) {
                self.pos += syntax.len();
                return Ok(string(value, start, self.pos));
            }
        }
        if c == '-' && self.rest()[1..].starts_with(char::is_numeric) {
            self.pos += 1;
            return Ok(string("\u{2212}", start, self.pos));
        }
        if c == '\'' || c == '"' {
            self.consume_char();
            let open = self.source[..start]
                .chars()
                .next_back()
                .is_none_or(|c| c.is_whitespace() || "([{\u{2018}\u{201c}".contains(c));
            return Ok(element(
                "smartquote",
                vec![
                    (
                        "double".into(),
                        expr(ExprKind::Bool(c == '"'), start, self.pos),
                    ),
                    ("open".into(), expr(ExprKind::Bool(open), start, self.pos)),
                ],
                start,
                self.pos,
            ));
        }
        self.consume_char();
        // Group ordinary characters to keep the AST proportional to words, not bytes.
        while let Some(next) = self.rest().chars().next() {
            if next.is_whitespace()
                || "#@[]*_\\`$\"'~-/.:".contains(next)
                || self.rest().starts_with("http")
            {
                break;
            }
            self.consume_char();
        }
        self.record(
            start,
            self.pos,
            "markup-text",
            &self.source[start..self.pos],
        );
        Ok(string(&self.source[start..self.pos], start, self.pos))
    }
    fn markup_raw(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let count = self.rest().bytes().take_while(|c| *c == b'`').count();
        self.pos += count;
        let mut lang = String::new();
        let mut text = String::new();
        if count != 2 {
            if count >= 3 {
                while let Some(c) = self
                    .rest()
                    .chars()
                    .next()
                    .filter(|c| c.is_alphanumeric() || "_-+".contains(*c))
                {
                    lang.push(c);
                    self.consume_char();
                }
            }
            let begin = self.pos;
            loop {
                if self.rest().is_empty() {
                    return Err("unclosed raw text".into());
                }
                let ticks = self.rest().bytes().take_while(|c| *c == b'`').count();
                if ticks == count {
                    text = self.source[begin..self.pos].to_owned();
                    self.pos += count;
                    break;
                }
                if ticks > 0 {
                    self.pos += ticks;
                } else {
                    self.consume_char();
                }
            }
        }
        let block = count >= 3 && text.contains('\n');
        if count >= 3 {
            if block {
                let mut lines = text
                    .split('\n')
                    .map(|line| line.trim_end_matches('\r'))
                    .collect::<Vec<_>>();
                let indent = lines
                    .iter()
                    .skip(1)
                    .filter(|line| !line.trim().is_empty())
                    .chain(lines.last())
                    .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
                    .min()
                    .unwrap_or(0);
                if lines.last().is_some_and(|s| s.trim().is_empty()) {
                    lines.pop();
                }
                let mut normalized = Vec::new();
                for (index, line) in lines.iter().enumerate() {
                    if index == 0 {
                        if !line.trim().is_empty() {
                            normalized.push(line.strip_prefix(' ').unwrap_or(line).to_string());
                        }
                    } else {
                        normalized.push(line.chars().skip(indent).collect::<String>());
                    }
                }
                text = normalized.join("\n");
            } else {
                if text.starts_with(' ') {
                    text.remove(0);
                }
                if text.ends_with("` ") {
                    text.pop();
                }
            }
        }
        self.record(start, self.pos, "raw", &self.source[start..self.pos]);
        Ok(element(
            "raw",
            vec![
                ("content".into(), string(text, start, self.pos)),
                ("lang".into(), string(lang, start, self.pos)),
                ("block".into(), expr(ExprKind::Bool(block), start, self.pos)),
            ],
            start,
            self.pos,
        ))
    }
    fn markup_list(&mut self, kind: String, end: Option<char>) -> Result<Expr, String> {
        let start = self.pos;
        let column = self.line_position().unwrap();
        let mut items = Vec::new();
        let mut tight = true;
        while let Some((name, length, number)) = self.marker() {
            if name != kind || self.line_position() != Some(column) {
                break;
            }
            let begin = self.pos;
            self.pos += length;
            while self.rest().starts_with([' ', '\t']) {
                self.consume_char();
            }
            let term = if kind == "terms" {
                Some(self.markup_inline(':', false)?)
            } else {
                None
            };
            let parts = self.markup_flow(end, 0, Some(column), true)?;
            let mut fields = vec![(
                "body".into(),
                expr(ExprKind::Content(parts), begin, self.pos),
            )];
            if let Some(term) = term {
                fields.push((
                    "term".into(),
                    expr(ExprKind::Content(term), begin, self.pos),
                ));
            }
            if let Some(number) = number {
                fields.push((
                    "number".into(),
                    expr(ExprKind::Int(number), begin, self.pos),
                ));
            }
            let item_name = if kind == "terms" {
                "term-item"
            } else {
                "list-item"
            };
            items.push(element(item_name, fields, begin, self.pos));
            if self.source[begin..self.pos].contains("\n\n") {
                tight = false;
            }
        }
        let name = if kind == "terms" { "terms" } else { "list" };
        Ok(element(
            name,
            vec![
                (
                    "body".into(),
                    expr(ExprKind::Content(items), start, self.pos),
                ),
                (
                    "ordered".into(),
                    expr(ExprKind::Bool(kind == "enum"), start, self.pos),
                ),
                ("tight".into(), expr(ExprKind::Bool(tight), start, self.pos)),
            ],
            start,
            self.pos,
        ))
    }
}
