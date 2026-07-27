//! Versioned local protocol contracts. Transport framing lives beside the daemon client.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_MAJOR: u16 = 3;
pub const PROTOCOL_MINOR: u16 = 1;
pub const CAPABILITIES: &[&str] = &[
    "completion",
    "definition",
    "diagnostics",
    "edit",
    "hover",
    "references",
    "search",
    "bounded_query",
    "read_source",
    "symbols",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const CURRENT: Self = Self {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Cli,
    Lsp,
    Mcp,
    Preview,
    Test,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Handshake {
    pub protocol_version: ProtocolVersion,
    pub client_kind: ClientKind,
    pub client_version: String,
    /// Canonical root this client expects the daemon to serve.
    pub vault_root: PathBuf,
    /// Optional generation identity for a managed vault such as official docs.
    #[serde(default)]
    pub vault_generation: Option<String>,
    pub requested_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HandshakeAccepted {
    pub protocol_version: ProtocolVersion,
    pub daemon_instance: String,
    pub capabilities: Vec<String>,
}

pub fn negotiate(handshake: &Handshake) -> Result<HandshakeAccepted, String> {
    if handshake.protocol_version.major != PROTOCOL_MAJOR {
        return Err(format!(
            "unsupported protocol major {}; daemon supports {}",
            handshake.protocol_version.major, PROTOCOL_MAJOR
        ));
    }
    Ok(HandshakeAccepted {
        protocol_version: ProtocolVersion {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
        },
        daemon_instance: String::new(),
        capabilities: handshake
            .requested_capabilities
            .iter()
            .filter(|capability| CAPABILITIES.contains(&capability.as_str()))
            .cloned()
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_major_and_negotiates_minor_capabilities() {
        let mut handshake = Handshake {
            protocol_version: ProtocolVersion::CURRENT,
            client_kind: ClientKind::Test,
            client_version: "test".into(),
            vault_root: PathBuf::from("/test"),
            vault_generation: None,
            requested_capabilities: vec!["diagnostics".into()],
        };
        assert_eq!(negotiate(&handshake).unwrap().capabilities, ["diagnostics"]);
        handshake.protocol_version.major += 1;
        assert!(negotiate(&handshake).is_err());
    }
}
