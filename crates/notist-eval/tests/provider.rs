use notist_eval::{ModuleProvider, Runtime};
use notist_syntax::{ParseResult, parse_traced};

struct Buffers {
    entry: ParseResult,
    library: ParseResult,
}

impl ModuleProvider for Buffers {
    fn parsed(&self, source: &str) -> Option<&ParseResult> {
        match source {
            "editor:1" => Some(&self.entry),
            "editor:2" => Some(&self.library),
            _ => None,
        }
    }
    fn module_key(&self, source: &str) -> Option<&str> {
        match source {
            "editor:1" => Some("app"),
            "editor:2" => Some("library"),
            _ => None,
        }
    }
    fn module_source(&self, key: &str) -> Option<&str> {
        match key {
            "app" => Some("editor:1"),
            "library" => Some("editor:2"),
            _ => None,
        }
    }
    fn dependency(&self, package: &str, alias: &str) -> Option<&str> {
        (package == "app" && alias == "dep").then_some("library")
    }
    fn binary(&self, _: &str) -> Option<&[u8]> {
        None
    }
    fn binary_path(&self, _: &str, _: &str) -> Result<String, String> {
        Err("this host does not provide binaries".into())
    }
    fn errors(&self) -> &[String] {
        &[]
    }
}

#[test]
fn evaluates_preparsed_buffers_without_filename_conventions() {
    let buffers = Buffers {
        entry: parse_traced("use dep::twice; twice(21);"),
        library: parse_traced("let twice = (x: Int) => x * 2;"),
    };
    let result = Runtime::new(&buffers).evaluate("editor:1");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.content.to_json()["sequence"][0]["text"], "42");
}
