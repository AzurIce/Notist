use notist_model::{ModuleReference, TextRange, WikiReference};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WikiLink {
    pub target: WikiReference,
    pub range: TextRange,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxError {
    pub message: String,
    pub range: TextRange,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Parse {
    pub links: Vec<WikiLink>,
    pub errors: Vec<SyntaxError>,
}

pub fn parse(source: &str) -> Parse {
    let mut result = Parse::default();
    let mut cursor = 0;

    while let Some(relative_start) = source[cursor..].find("[[") {
        let start = cursor + relative_start;
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

    let segments: Vec<_> = source.split("::").collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return Err("module path contains an empty segment".into());
    }
    if let Some(segment) = segments.iter().find(|segment| !is_identifier(segment)) {
        return Err(format!("invalid module path segment `{segment}`"));
    }

    if segments[0] == "vault" {
        if segments[1..]
            .iter()
            .any(|segment| *segment == "vault" || *segment == "super")
        {
            return Err("`vault` and `super` are only allowed at the start of a path".into());
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
            .any(|segment| *segment == "vault" || *segment == "super")
        {
            return Err("`vault` and `super` are only allowed at the start of a path".into());
        }
        return Ok(ModuleReference::Parent {
            levels,
            remainder: segments[levels..]
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect(),
        });
    }

    if segments
        .iter()
        .any(|segment| *segment == "vault" || *segment == "super")
    {
        return Err("`vault` and `super` are reserved path segments".into());
    }

    Ok(ModuleReference::Relative(
        segments
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect(),
    ))
}

fn is_identifier(source: &str) -> bool {
    source
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
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
    fn parses_label_syntax_without_resolving_it() {
        let reference = parse_wiki_reference("pages::intro#section").unwrap();
        assert_eq!(reference.label.as_deref(), Some("section"));
    }

    #[test]
    fn reports_invalid_paths() {
        let parse = parse("[[]] [[foo::::bar]] [[foo::super]]");
        assert_eq!(parse.errors.len(), 3);
    }
}
