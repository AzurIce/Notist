use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use turn::{
    Error,
    auth::{AuthHandler, generate_auth_key},
};

pub fn equal(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct TurnAuth {
    secret: Vec<u8>,
}
impl Default for TurnAuth {
    fn default() -> Self {
        Self {
            secret: [
                uuid::Uuid::new_v4().as_bytes().as_slice(),
                uuid::Uuid::new_v4().as_bytes().as_slice(),
            ]
            .concat(),
        }
    }
}
impl TurnAuth {
    pub fn credentials(&self) -> (String, String) {
        let username = format!("{}:{}", now() + 600, uuid::Uuid::new_v4());
        let password = self.password(&username);
        (username, password)
    }
    fn password(&self, username: &str) -> String {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(&self.secret).expect("HMAC accepts arbitrary keys");
        mac.update(username.as_bytes());
        STANDARD.encode(mac.finalize().into_bytes())
    }
}
impl AuthHandler for TurnAuth {
    fn auth_handle(
        &self,
        username: &str,
        realm: &str,
        _: SocketAddr,
    ) -> std::result::Result<Vec<u8>, turn::Error> {
        let expiry = username
            .split(':')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if username.len() > 128 || expiry <= now() || expiry > now() + 601 {
            return Err(Error::ErrNoSuchUser);
        }
        Ok(generate_auth_key(username, realm, &self.password(username)))
    }
}
