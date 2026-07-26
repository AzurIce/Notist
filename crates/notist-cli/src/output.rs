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

/// Writes one human-readable diagnostic line without depending on the active
/// Windows console code page. Redirected output remains UTF-8.
pub(crate) fn emit_text_error(message: &str) {
    #[cfg(windows)]
    if windows_console::write_stderr_line(message) {
        return;
    }

    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(message.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

#[cfg(windows)]
mod windows_console {
    use std::ffi::c_void;

    type Handle = *mut c_void;

    const STD_ERROR_HANDLE: u32 = -12i32 as u32;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> Handle;
        fn GetConsoleMode(console_handle: Handle, mode: *mut u32) -> i32;
        fn WriteConsoleW(
            console_output: Handle,
            buffer: *const u16,
            characters: u32,
            written: *mut u32,
            reserved: *mut c_void,
        ) -> i32;
    }

    pub(super) fn write_stderr_line(message: &str) -> bool {
        let handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if handle.is_null() {
            return false;
        }
        let mut mode = 0;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return false;
        }
        let mut wide: Vec<u16> = message.encode_utf16().collect();
        wide.push('\r' as u16);
        wide.push('\n' as u16);
        for chunk in wide.chunks(u32::MAX as usize) {
            let mut written = 0;
            if unsafe {
                WriteConsoleW(
                    handle,
                    chunk.as_ptr(),
                    chunk.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            } == 0
                || written != chunk.len() as u32
            {
                return false;
            }
        }
        true
    }
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
