use super::*;
use syn::parse::Parser;

fn expand(attributes: &str, function: &str) -> syn::Result<Tokens> {
    expand_func(
        syn::parse_str(attributes)?,
        syn::parse_str(function)?,
        &quote!(::sdk),
    )
}

#[test]
fn attributes_accept_rust_expressions_and_reject_ambiguous_defaults() {
    let output = expand(
        "defaults(value = Some(2),)",
        "pub fn echo(value: Option<i64>) -> Option<i64> { value }",
    )
    .unwrap();
    syn::parse2::<syn::File>(output).unwrap();
    for (attrs, expected) in [
        (
            "defaults(value = 1, value = 2)",
            "duplicate parameter default",
        ),
        ("defaults(missing = 1)", "unknown parameter"),
        ("unexpected", "expected defaults"),
    ] {
        assert!(
            expand(attrs, "fn echo(value: i64) -> i64 { value }")
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    assert!(
        expand(
            "defaults(value = 1) extra",
            "fn echo(value: i64) -> i64 { value }"
        )
        .is_err()
    );
}

#[test]
fn unsupported_signatures_have_direct_errors() {
    for signature in [
        "async fn echo(value: i64) -> i64 { value }",
        "unsafe fn echo(value: i64) -> i64 { value }",
        "fn echo<T>(value: T) -> T { value }",
        "extern \"C\" fn echo(value: i64) -> i64 { value }",
    ] {
        assert!(
            expand("", signature)
                .unwrap_err()
                .to_string()
                .contains("synchronous, safe, non-generic")
        );
    }
    assert!(
        expand("", "fn echo((value,): (i64,)) -> i64 { value }")
            .unwrap_err()
            .to_string()
            .contains("identifier names")
    );
    assert!(
        expand("", "fn echo(&self) {}")
            .unwrap_err()
            .to_string()
            .contains("free functions")
    );
    assert!(
        expand("", "fn alloc(value: i64) -> i64 { value }")
            .unwrap_err()
            .to_string()
            .contains("reserved")
    );
}

#[test]
fn initialization_accepts_paths_and_rejects_duplicate_names() {
    let parse = Punctuated::<Path, Token![,]>::parse_terminated;
    let expanded = expand_init(
        parse.parse_str("echo, nested::other,").unwrap(),
        None,
        &quote!(::sdk),
    )
    .unwrap();
    syn::parse2::<syn::File>(expanded).unwrap();
    assert!(
        expand_init(
            parse.parse_str("one::echo, two::echo").unwrap(),
            None,
            &quote!(::sdk)
        )
        .unwrap_err()
        .to_string()
        .contains("duplicate exported")
    );
    assert!(
        expand_init(
            parse.parse_str("echo::<i64>").unwrap(),
            None,
            &quote!(::sdk)
        )
        .unwrap_err()
        .to_string()
        .contains("without generic arguments")
    );
}
