use crate::document::DocumentIdentity;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub state_dir: PathBuf,
    pub access_token: String,
    pub http: HttpConfig,
    #[serde(default)]
    pub network_service: NetworkServiceConfig,
    #[serde(default)]
    pub peer: PeerConfig,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    pub listen: SocketAddr,
    pub public_url: String,
    pub tls: Option<TlsConfig>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkServiceConfig {
    #[serde(default)]
    pub enabled: bool,
    pub turn: Option<TurnConfig>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TurnConfig {
    pub listen_udp: SocketAddr,
    pub listen_tcp: SocketAddr,
    pub listen_tls: Option<SocketAddr>,
    pub public_ip: IpAddr,
    pub public_host: String,
    pub relay_ports: [u16; 2],
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub documents: Vec<DocumentConfig>,
    #[serde(default)]
    pub connect: Vec<RemoteConfig>,
    #[serde(default)]
    pub signaling: Vec<RemoteConfig>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentConfig {
    pub identity: DocumentIdentity,
    pub credential: String,
    #[serde(default)]
    pub initial_text: String,
    /// Optional UTF-8 text output. Relative to the configuration file.
    pub text_file: Option<PathBuf>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub url: String,
    pub access_token: String,
}

impl Config {
    pub fn read(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(&std::fs::read_to_string(path)?)?;
        let base = path.parent().unwrap_or(Path::new("."));
        if config.state_dir.is_relative() {
            config.state_dir = base.join(&config.state_dir);
        }
        if let Some(tls) = &mut config.http.tls {
            if tls.certificate.is_relative() {
                tls.certificate = base.join(&tls.certificate);
            }
            if tls.private_key.is_relative() {
                tls.private_key = base.join(&tls.private_key);
            }
        }
        for document in &mut config.peer.documents {
            if let Some(path) = &mut document.text_file
                && path.is_relative()
            {
                *path = base.join(&*path);
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.access_token.len() < 24 {
            bail!("access_token must contain at least 24 bytes");
        }
        if !self.network_service.enabled && !self.peer.enabled {
            bail!("enable network_service or peer");
        }
        if self.http.tls.is_none() && !self.http.listen.ip().is_loopback() {
            bail!(
                "public HTTP listeners require TLS; plaintext is limited to loopback development"
            );
        }
        if !(self.http.public_url.starts_with("https://")
            || (self.http.tls.is_none() && self.http.public_url.starts_with("http://")))
        {
            bail!("public_url scheme does not match TLS configuration");
        }
        if self.network_service.enabled {
            let t =
                self.network_service.turn.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("network_service requires TURN configuration")
                })?;
            if t.relay_ports[0] == 0
                || t.relay_ports[1] < t.relay_ports[0]
                || t.relay_ports[1] - t.relay_ports[0] > 4095
            {
                bail!("invalid or excessive TURN relay port range");
            }
            if t.public_ip.is_unspecified() || t.public_host.is_empty() {
                bail!("TURN requires a public address and host");
            }
            if t.listen_tls.is_some() && self.http.tls.is_none() {
                bail!("TURN TLS requires certificate configuration");
            }
            if !t.public_ip.is_loopback() && t.listen_tls.is_none() {
                bail!("public network_service requires TURN TLS as well as UDP/TCP");
            }
        }
        if !self.peer.enabled
            && (!self.peer.documents.is_empty()
                || !self.peer.connect.is_empty()
                || !self.peer.signaling.is_empty())
        {
            bail!("disabled peer cannot have documents or outgoing connections");
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut text_files = std::collections::BTreeSet::new();
        for d in &self.peer.documents {
            if d.identity.document_id.is_empty()
                || d.identity.history_id.is_empty()
                || !ids.insert(&d.identity.document_id)
                || d.credential.len() < 24
            {
                bail!("invalid document identity or credential");
            }
            if let Some(path) = &d.text_file
                && (path.file_name().is_none() || !text_files.insert(path))
            {
                bail!("invalid or duplicate text_file path");
            }
        }
        Ok(())
    }
}
