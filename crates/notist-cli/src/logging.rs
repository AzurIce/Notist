//! Process-wide `tracing` subscriber installation.
//!
//! Tracing stays silent unless explicitly enabled. `NOTIST_LOG` accepts a
//! bare level (`debug`, `trace`, ...) which is scoped to the `notist_*`
//! targets so Wasmtime/Cranelift internals stay quiet, or any full
//! `EnvFilter` directive list (`notist_plugin_host=trace,info`). When
//! neither `NOTIST_LOG` nor `RUST_LOG` is set no events are emitted.

/// Installs the global tracing subscriber from the environment.
///
/// Daemon children inherit this process's environment, so a daemon spawned
/// from an enabled CLI emits the same event stream on its own stderr.
pub fn init_from_env() {
    let filter = match std::env::var("NOTIST_LOG") {
        Ok(filter) => {
            if filter.contains('=') || filter.contains(',') {
                filter
            } else {
                format!("notist_={filter}")
            }
        }
        Err(_) => std::env::var("RUST_LOG").unwrap_or_else(|_| "off".to_owned()),
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_target(true)
        .try_init();
}
