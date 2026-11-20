//! Share links through your own relay (relay/ in this repo, deployed to your
//! Cloudflare account). The CLI holds the relay's URL and publish secret in
//! `<home>/relay.json`, and the links it made in `<home>/shares.json`, so
//! sharing again updates the same link and `unshare` can revoke it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
pub struct Relay {
    pub url: String,
    pub secret: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Link {
    pub token: String,
    pub url: String,
    pub expires_at: u64,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).expect("serializes");
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// A random token of `len` characters from [A-Za-z0-9]: 22 give about 131 bits.
pub fn token(len: usize) -> Result<String, String> {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).map_err(|e| format!("could not make a token: {e}"))?;
        // 248 = 4 * 62: rejecting bytes above it keeps every character equally likely.
        out.extend(buf.iter().filter(|&&b| b < 248).map(|&b| ABC[(b % 62) as usize] as char).take(len - out.len()));
    }
    Ok(out)
}

pub struct Home(pub PathBuf);

impl Home {
    pub fn relay(&self) -> Result<Relay, String> {
        read_json(&self.0.join("relay.json"))?.ok_or_else(|| {
            "no relay set up. Deploy relay/ to your Cloudflare account, then `jelly relay URL` (see relay/README.md)".into()
        })
    }

    /// Saves the relay URL with a new publish secret, and returns the secret.
    pub fn set_relay(&self, url: &str) -> Result<String, String> {
        let url = url.trim_end_matches('/').to_string();
        url::Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
        let secret = token(43)?;
        write_json(&self.0.join("relay.json"), &Relay { url, secret: secret.clone() })?;
        Ok(secret)
    }

    fn links(&self) -> Result<BTreeMap<String, Link>, String> {
        Ok(read_json(&self.0.join("shares.json"))?.unwrap_or_default())
    }

    pub fn link(&self, jelly: &str) -> Result<Option<Link>, String> {
        Ok(self.links()?.remove(jelly))
    }

    pub fn save_link(&self, jelly: &str, link: Option<Link>) -> Result<(), String> {
        let mut all = self.links()?;
        match link {
            Some(l) => all.insert(jelly.to_string(), l),
            None => all.remove(jelly),
        };
        write_json(&self.0.join("shares.json"), &all)
    }
}

fn relay_error(e: ureq::Error, relay: &Relay) -> String {
    match e {
        ureq::Error::Status(401, _) => format!("{} refused the publish secret; run `jelly relay {}` and set the new secret on the relay", relay.url, relay.url),
        ureq::Error::Status(code, resp) => format!("{}: {code} {}", relay.url, resp.into_string().unwrap_or_default().trim()),
        ureq::Error::Transport(t) => format!("cannot reach the relay at {} ({t})", relay.url),
    }
}

/// Publishes `html` under `token` for `ttl_s` seconds; returns when it expires (ms).
pub fn publish(relay: &Relay, token: &str, html: &str, ttl_s: u64) -> Result<u64, String> {
    let resp: serde_json::Value = ureq::put(&format!("{}/api/shares/{token}?ttl={ttl_s}", relay.url))
        .set("Authorization", &format!("Bearer {}", relay.secret))
        .set("Content-Type", "text/html; charset=utf-8")
        .send_string(html)
        .map_err(|e| relay_error(e, relay))?
        .into_json()
        .map_err(|e| e.to_string())?;
    resp["expires_at"].as_u64().ok_or_else(|| format!("{}: unexpected answer {resp}", relay.url))
}

pub fn revoke(relay: &Relay, token: &str) -> Result<(), String> {
    ureq::delete(&format!("{}/api/shares/{token}", relay.url))
        .set("Authorization", &format!("Bearer {}", relay.secret))
        .call()
        .map_err(|e| relay_error(e, relay))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_alphanumeric_and_distinct() {
        let a = token(22).unwrap();
        assert_eq!(a.len(), 22);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, token(22).unwrap());
    }
}
