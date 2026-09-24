//! Platform-independent peer state machine. Hosts execute transport and storage
//! effects, and report durable commits explicitly. No sockets, clocks or tasks.
mod peer;
pub use notist_editor_document as document;
pub use peer::*;

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod native;
