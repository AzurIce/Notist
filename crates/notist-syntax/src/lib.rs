use notist_model::{ModuleReference, TextRange, WikiReference};

mod argument;
mod raw;
mod scope;

pub use argument::{
    Argument, Expression, ExpressionKind, StringLiteral, StringLiteralForm, StringLiteralStyle,
};
pub use raw::{RawLiteral, RawLiteralForm, SpannedText};
pub use scope::{
    Attribute, AttributeValue, Attributes, BodyForm, Call, ContentBody, SpannedName,
    TransparentScope,
};

/// A parsed wiki-style module or label reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiLink {
    /// The structured reference target.
    pub target: WikiReference,
    /// The complete source range including `[[` and `]]`.
    pub range: TextRange,
}

/// A recoverable syntax error with a precise source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxError {
    /// A user-facing description of the syntax error.
    pub message: String,
    /// The source range associated with the error.
    pub range: TextRange,
}

/// The syntax information currently extracted from a Notist source file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parse {
    /// Wiki-style references visible to the host parser.
    pub links: Vec<WikiLink>,
    /// Transparent annotation scopes discovered in source order.
    pub scopes: Vec<TransparentScope>,
    /// Function calls discovered in source order.
    pub calls: Vec<Call>,
    /// Host-visible inline and fenced raw source literals.
    pub raw_literals: Vec<RawLiteral>,
    /// Recoverable errors produced during parsing.
    pub errors: Vec<SyntaxError>,
}

/// Parses the supported Notist syntax from a source string.
pub fn parse(source: &str) -> Parse {
    let mut result = Parse::default();
    let raw_parse = raw::parse_raw_literals(source);
    (result.scopes, result.calls) =
        scope::parse_scopes_and_calls(source, &raw_parse.literals, &mut result.errors);
    result.raw_literals = raw_parse
        .literals
        .iter()
        .filter(|literal| {
            !position_is_in_embedded_value_syntax(
                &result.scopes,
                &result.calls,
                literal.range.start,
            )
        })
        .cloned()
        .collect();
    result
        .errors
        .extend(raw_parse.errors.into_iter().filter(|error| {
            !position_is_in_embedded_value_syntax(&result.scopes, &result.calls, error.range.start)
        }));
    let mut cursor = 0;

    while let Some(relative_start) = source[cursor..].find("[[") {
        let start = cursor + relative_start;
        if let Some(raw) = raw::containing(&result.raw_literals, start) {
            cursor = raw.range.end;
            continue;
        }
        if let Some(hidden_end) = hidden_syntax_end(&result, start) {
            cursor = hidden_end;
            continue;
        }
        let content_start = start + 2;
        let Some(relative_end) = source[content_start..].find("]]") else {
            result.errors.push(SyntaxError {
                message: "unclosed wiki reference".into(),
                range: TextRange::new(start, source.len()),
            });
            break;
        };
        let content_end = content_start + relative_end;
        let end = content_end + 2;
        let range = TextRange::new(start, end);

        match parse_wiki_reference(&source[content_start..content_end]) {
            Ok(target) => result.links.push(WikiLink { target, range }),
            Err(message) => result.errors.push(SyntaxError { message, range }),
        }
        cursor = end;
    }

    result
}

fn position_is_in_embedded_value_syntax(
    scopes: &[TransparentScope],
    calls: &[Call],
    position: usize,
) -> bool {
    let contains = |range: TextRange| range.start <= position && position < range.end;
    scopes
        .iter()
        .any(|scope| scope.attributes.range.is_some_and(contains))
        || calls.iter().any(|call| {
            call.arguments_range.is_some_and(contains)
                || call.attributes.range.is_some_and(contains)
        })
}

fn hidden_syntax_end(parse: &Parse, position: usize) -> Option<usize> {
    let scope_end = parse.scopes.iter().find_map(|scope| {
        if scope.range.start <= position && position < scope.body_range.start {
            Some(scope.body_range.start)
        } else if scope.body_range.end <= position && position < scope.range.end {
            Some(scope.range.end)
        } else {
            None
        }
    });
    let call_end = parse.calls.iter().find_map(|call| match call.body {
        Some(body) if call.range.start <= position && position < body.payload_range.start => {
            Some(body.payload_range.start)
        }
        Some(body) if body.payload_range.end <= position && position < call.range.end => {
            Some(call.range.end)
        }
        None if call.range.start <= position && position < call.range.end => Some(call.range.end),
        _ => None,
    });
    match (scope_end, call_end) {
        (Some(scope), Some(call)) => Some(scope.min(call)),
        (Some(end), None) | (None, Some(end)) => Some(end),
        (None, None) => None,
    }
}

/// Parses a wiki reference body without the surrounding `[[` and `]]`.
pub fn parse_wiki_reference(source: &str) -> Result<WikiReference, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("wiki reference cannot be empty".into());
    }

    let mut parts = source.split('#');
    let module_part = parts.next().unwrap_or_default();
    let label = parts.next().map(str::trim).map(str::to_owned);
    if parts.next().is_some() {
        return Err("wiki reference contains more than one `#`".into());
    }
    if label.as_deref() == Some("") {
        return Err("wiki reference label cannot be empty".into());
    }

    let module = parse_module_reference(module_part.trim(), label.is_some())?;
    Ok(WikiReference { module, label })
}

fn parse_module_reference(source: &str, has_label: bool) -> Result<ModuleReference, String> {
    if source.is_empty() {
        if has_label {
            return Ok(ModuleReference::Relative(Vec::new()));
        }
        return Err("module path cannot be empty".into());
    }

    let segments: Vec<_> = source.split("::").map(str::trim).collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err("module path contains an empty segment".into());
    }
    if let Some(segment) = segments.iter().find(|segment| !is_module_segment(segment)) {
        return Err(format!("invalid module path segment `{segment}`"));
    }

    if segments[0] == "vault" {
        if segments[1..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Absolute(
            segments[1..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        ));
    }

    if segments[0] == "super" {
        let levels = segments
            .iter()
            .take_while(|segment| **segment == "super")
            .count();
        if segments[levels..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Parent {
            levels,
            remainder: segments[levels..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        });
    }

    if segments[0] == "self" {
        if segments[1..]
            .iter()
            .any(|segment| matches!(*segment, "vault" | "super" | "self"))
        {
            return Err(
                "`vault`, `super`, and `self` are only allowed at the start of a path".into(),
            );
        }
        return Ok(ModuleReference::Relative(
            segments[1..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        ));
    }

    if segments
        .iter()
        .any(|segment| matches!(*segment, "vault" | "super" | "self"))
    {
        return Err("`vault`, `super`, and `self` are reserved path segments".into());
    }

    Ok(ModuleReference::Relative(
        segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
    ))
}

fn is_module_segment(source: &str) -> bool {
    !source.chars().any(|character| {
        character.is_control() || matches!(character, '/' | '\\' | '#' | '[' | ']')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_module_references() {
        let parse = parse("[[intro]] [[pages::intro]] [[super::guide]] [[vault::index]]");
        assert!(parse.errors.is_empty());
        assert_eq!(parse.links.len(), 4);
        assert_eq!(
            parse.links[2].target.module,
            ModuleReference::Parent {
                levels: 1,
                remainder: vec!["guide".into()],
            }
        );
    }

    #[test]
    fn parses_module_paths_with_spaces_and_explicit_self() {
        let reference =
            parse_wiki_reference("self::2026-07-11 typst element function and syntax sugar")
                .unwrap();
        assert_eq!(
            reference.module,
            ModuleReference::Relative(vec![
                "2026-07-11 typst element function and syntax sugar".into()
            ])
        );

        let current = parse_wiki_reference("self").unwrap();
        assert_eq!(current.module, ModuleReference::Relative(Vec::new()));
    }

    #[test]
    fn rejects_unsafe_module_path_characters() {
        for source in ["foo/bar", "foo\\bar", "foo[bar]"] {
            assert!(parse_wiki_reference(source).is_err(), "{source}");
        }
    }

    #[test]
    fn parses_label_syntax_without_resolving_it() {
        let reference = parse_wiki_reference("pages::intro#section").unwrap();
        assert_eq!(reference.label.as_deref(), Some("section"));
    }

    #[test]
    fn reports_invalid_paths() {
        let parse = parse("[[]] [[foo::::bar]] [[foo::super]]");
        assert_eq!(parse.errors.len(), 3);
    }

    #[test]
    fn parses_links_in_content_calls_but_ignores_them_in_call_arguments() {
        let parse =
            parse("[[outside]] #quote[[[inside]]] #code(source=r#\"[[[raw]]]\"#) [[after]]");
        assert!(parse.errors.is_empty());
        assert_eq!(parse.links.len(), 3);
        assert_eq!(parse.calls.len(), 2);
    }

    #[test]
    fn transparent_delimiters_do_not_overlap_wiki_reference_markers() {
        let parse = parse("#[[[self::target]]]@concept");
        assert!(parse.errors.is_empty());
        assert_eq!(parse.links.len(), 1);
        assert_eq!(
            parse.links[0].target.module,
            ModuleReference::Relative(vec!["target".into()])
        );
    }

    #[test]
    fn ignores_syntax_inside_raw_content() {
        let parse = parse("`[[inline]] #[annotation]`\n```not\n#code()[[[inside]]]\n```");
        assert!(parse.errors.is_empty());
        assert!(parse.links.is_empty());
        assert!(parse.scopes.is_empty());
        assert!(parse.calls.is_empty());
        assert_eq!(parse.raw_literals.len(), 2);
    }

    #[test]
    fn backticks_inside_string_arguments_are_not_host_raw_literals() {
        let parse = parse(r###"#code(value=r#"`"#) [[after]]"###);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert!(parse.raw_literals.is_empty());
        assert_eq!(parse.links.len(), 1);
    }

    #[test]
    fn rejects_backtick_literals_in_argument_expressions() {
        let parse = parse("#code(value=`text`)");
        assert_eq!(parse.calls.len(), 1);
        assert!(parse.raw_literals.is_empty());
        assert_eq!(parse.errors.len(), 1);
        assert!(
            parse.errors[0]
                .message
                .contains("not supported in argument expressions")
        );
    }

    #[test]
    fn backticks_inside_transparent_scope_attributes_are_not_host_raw_literals() {
        let parse = parse("#[body]@title=\"`\",sample=\"`paired`\"");

        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert!(parse.raw_literals.is_empty());
        assert_eq!(parse.scopes.len(), 1);
        assert_eq!(parse.scopes[0].attributes.items.len(), 2);
    }
}
