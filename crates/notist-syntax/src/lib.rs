use notist_model::{ModuleReference, TextRange, WikiReference};

mod raw;
mod scope;

pub use scope::{
    Attribute, AttributeValue, Attributes, OpaqueScope, Scope, SpannedName, TransparentScope,
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Parse {
    /// Wiki-style references visible to the host parser.
    pub links: Vec<WikiLink>,
    /// Transparent and opaque scopes discovered in source order.
    pub scopes: Vec<Scope>,
    /// Recoverable errors produced during parsing.
    pub errors: Vec<SyntaxError>,
}

/// Parses the supported Notist syntax from a source string.
pub fn parse(source: &str) -> Parse {
    let mut result = Parse::default();
    let raw_ranges = raw::raw_ranges(source);
    result.scopes = scope::parse_scopes(source, &raw_ranges, &mut result.errors);
    let mut cursor = 0;

    while let Some(relative_start) = source[cursor..].find("[[") {
        let start = cursor + relative_start;
        if let Some(raw) = raw::containing(&raw_ranges, start) {
            cursor = raw.end;
            continue;
        }
        if let Some(opaque) = opaque_containing(&result.scopes, start) {
            cursor = opaque.range.end;
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

fn opaque_containing(scopes: &[Scope], position: usize) -> Option<&OpaqueScope> {
    scopes.iter().find_map(|scope| match scope {
        Scope::Opaque(opaque) if opaque.range.start <= position && position < opaque.range.end => {
            Some(opaque)
        }
        _ => None,
    })
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
    fn ignores_wiki_references_inside_opaque_scopes() {
        let parse = parse("[[outside]] #code[[[inside]]] [[after]]");
        assert!(parse.errors.is_empty());
        assert_eq!(parse.links.len(), 2);
        assert_eq!(parse.scopes.len(), 1);
    }

    #[test]
    fn ignores_syntax_inside_raw_content() {
        let parse = parse("`[[inline]] #[annotation]`\n```not\n#code[[[inside]]]\n```");
        assert!(parse.errors.is_empty());
        assert!(parse.links.is_empty());
        assert!(parse.scopes.is_empty());
    }
}
