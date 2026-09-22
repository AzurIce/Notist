use crate::{Content, Diagnostic, DiagnosticCode, Item, Value};
impl Value {
    /// Inspect errors in a value without giving dictionaries Item semantics.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = vec![];
        value(self, &mut out);
        out
    }
}
impl Content {
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = vec![];
        content(self, &mut out);
        out
    }
}
fn item(i: &Item, out: &mut Vec<Diagnostic>) {
    if let Err(message) = i.label() {
        out.push(Diagnostic::error(
            DiagnosticCode::InvalidLabel,
            message,
            Some(i.location.clone()),
        ));
    }
    for v in i.args.values().chain(i.attributes.values()) {
        value(v, out);
    }
}
fn value(v: &Value, out: &mut Vec<Diagnostic>) {
    match v {
        Value::Content(c) => content(c, out),
        Value::List(v) => {
            for v in v {
                value(v, out);
            }
        }
        Value::Dict(v) => {
            for v in v.values() {
                value(v, out);
            }
        }
        _ => {}
    }
}
fn content(c: &Content, out: &mut Vec<Diagnostic>) {
    if c.name == "error" {
        let code = c
            .string("code")
            .and_then(|s| serde_json::from_value(serde_json::json!(s)).ok())
            .unwrap_or(DiagnosticCode::Evaluation);
        let mut diagnostic = Diagnostic::error(
            code,
            c.string("message").unwrap_or("invalid error Item"),
            Some(c.location.clone()),
        );
        diagnostic.span = c.span.clone();
        out.push(diagnostic);
    }
    item(c, out);
}
