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
    schema_version: u32,
    command: &'a str,
    ok: bool,
    result: T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    schema_version: u32,
    command: &'a str,
    ok: bool,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    message: &'a str,
}

#[derive(Serialize)]
struct EventEnvelope<'a, T> {
    schema_version: u32,
    command: &'a str,
    event: &'a str,
    data: T,
}

pub(crate) fn emit_result<T: Serialize>(command: &str, ok: bool, result: T) -> io::Result<()> {
    write_json(
        io::stdout().lock(),
        &ResultEnvelope {
            schema_version: 1,
            command,
            ok,
            result,
        },
    )
}

pub(crate) fn emit_error(command: &str, message: &str) -> io::Result<()> {
    write_json(
        io::stderr().lock(),
        &ErrorEnvelope {
            schema_version: 1,
            command,
            ok: false,
            error: ErrorBody { message },
        },
    )
}

pub(crate) fn emit_event<T: Serialize>(command: &str, event: &str, data: T) -> io::Result<()> {
    write_json(
        io::stdout().lock(),
        &EventEnvelope {
            schema_version: 1,
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
            schema_version: 1,
            command: "search",
            ok: true,
            result: serde_json::json!({"results": []}),
        })
        .unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["command"], "search");
        assert_eq!(value["ok"], true);
        assert_eq!(value["result"]["results"], serde_json::json!([]));
    }
}
