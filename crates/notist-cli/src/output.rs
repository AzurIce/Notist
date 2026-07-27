use std::io::{self, Write};

use clap::ValueEnum;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl OutputFormat {
    pub(crate) fn is_json(self) -> bool {
        self == Self::Json
    }
}

#[derive(Serialize)]
struct ResultEnvelope<'a, T> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    command: &'a str,
    ok: bool,
    result: T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    command: &'a str,
    ok: bool,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
    retryable: bool,
    hint: Option<&'a str>,
    #[serde(skip_serializing_if = "slice_is_empty")]
    candidates: &'a [String],
}

fn slice_is_empty<T>(value: &[T]) -> bool {
    value.is_empty()
}

#[derive(Serialize)]
struct EventEnvelope<'a, T> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    command: &'a str,
    event: &'a str,
    data: T,
}

pub(crate) fn emit_result<T: Serialize>(command: &str, ok: bool, result: T) -> io::Result<()> {
    write_json(
        io::stdout().lock(),
        &ResultEnvelope {
            schema_version: 2,
            command,
            ok,
            result,
        },
    )
}

pub(crate) fn emit_typed_error(
    command: &str,
    code: &str,
    message: &str,
    retryable: bool,
    hint: Option<&str>,
    candidates: &[String],
) -> io::Result<()> {
    write_json(
        io::stderr().lock(),
        &ErrorEnvelope {
            schema_version: 2,
            command,
            ok: false,
            error: ErrorBody {
                code,
                message,
                retryable,
                hint,
                candidates,
            },
        },
    )
}

pub(crate) fn emit_event<T: Serialize>(command: &str, event: &str, data: T) -> io::Result<()> {
    write_json(
        io::stdout().lock(),
        &EventEnvelope {
            schema_version: 2,
            command,
            event,
            data,
        },
    )
}

fn write_json(mut writer: impl Write, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_envelope_has_a_versioned_stable_shape() {
        let value = serde_json::to_value(ResultEnvelope {
            schema_version: 2,
            command: "search",
            ok: true,
            result: serde_json::json!({"results": []}),
        })
        .unwrap();
        assert_eq!(value["schemaVersion"], 2);
        assert!(value.get("schema_version").is_none());
        assert_eq!(value["command"], "search");
        assert_eq!(value["ok"], true);
        assert_eq!(value["result"]["results"], serde_json::json!([]));
    }

    #[test]
    fn error_envelope_preserves_recovery_candidates() {
        let candidates = vec!["vault::one".into(), "vault::two".into()];
        let value = serde_json::to_value(ErrorEnvelope {
            schema_version: 2,
            command: "read",
            ok: false,
            error: ErrorBody {
                code: "ambiguous_selector",
                message: "selector is ambiguous",
                retryable: false,
                hint: Some("choose one candidate"),
                candidates: &candidates,
            },
        })
        .unwrap();
        assert_eq!(value["error"]["candidates"], serde_json::json!(candidates));
    }
}
