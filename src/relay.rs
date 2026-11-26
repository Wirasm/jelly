//! Your relay (relay/ in this repo, deployed to your Cloudflare account) and
//! what the daemon keeps about the jellies shared through it. The relay's URL
//! and secret are in `<home>/relay.json`; each share's record, with the links it
//! minted, in `<home>/shares/<share id>.json`, so shares survive restarts.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use crate::encode;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone)]
pub struct Relay {
    pub url: String,
    pub secret: String,
}

/// One shared jelly. While it exists, the relay's copy is the live one.
#[derive(Serialize, Deserialize, Clone)]
pub struct Record {
    pub jelly: String,
    /// The relay's name for it: the first half of every link.
    pub share: String,
    /// The last of the relay's events mirrored into the local store.
    pub seq: u64,
    /// Each link's secret by whom it is for; "" is the view-only link.
    pub links: BTreeMap<String, String>,
    /// When the share ends, as the relay said when a link or a PIN was last
    /// made, so the jelly page can say how long is left without asking it.
    #[serde(default)]
    pub expires_at: Option<u64>,
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

/// The share id of a link (`https://host/j/<share id><secret>`), or of a bare
/// share id: the 22 characters after `/j/`. It names the share on the relay
/// and opens nothing, so it is safe to pass around.
pub fn share_of(link: &str) -> Result<String, String> {
    let token = link.split_once("/j/").map_or(link, |(_, t)| t);
    let id: String = token.chars().take(22).collect();
    if id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id)
    } else {
        Err(format!("{link} is not a share link (https://<relay>/j/<44 characters>) or a share id (22 characters)"))
    }
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

    pub fn records(&self) -> Result<Vec<Record>, String> {
        let dir = self.0.join("shares");
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|x| x == "json") {
                out.extend(read_json::<Record>(&entry.path())?);
            }
        }
        Ok(out)
    }

    pub fn save(&self, r: &Record) -> Result<(), String> {
        write_json(&self.0.join("shares").join(format!("{}.json", r.share)), r)
    }

    pub fn forget(&self, share: &str) -> Result<(), String> {
        let path = self.0.join("shares").join(format!("{share}.json"));
        std::fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(format!("{}: {e}", path.display())) })
    }
}

/// Why the relay did not do something.
#[derive(Debug)]
pub enum Fail {
    /// It refused the change, and says why (a verb threw, no such ask).
    Refused(String),
    /// The share is over: unshared, or expired.
    Gone,
    Other(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fail::Refused(m) | Fail::Other(m) => f.write_str(m),
            Fail::Gone => f.write_str("the share has ended"),
        }
    }
}

impl Relay {
    fn at(&self, method: &str, share: &str, path: &str) -> ureq::Request {
        ureq::request(method, &format!("{}/api/live/{share}{path}", self.url)).set("Authorization", &format!("Bearer {}", self.secret))
    }

    fn fail(&self, e: ureq::Error) -> Fail {
        match e {
            ureq::Error::Status(401, _) => Fail::Other(format!("{} refused the publish secret; run `jelly relay {}` and set the new secret on the relay", self.url, self.url)),
            ureq::Error::Status(404, _) => Fail::Gone,
            ureq::Error::Status(400, resp) => {
                let body: Value = resp.into_json().unwrap_or_default();
                Fail::Refused(body["error"].as_str().unwrap_or("the relay refused the change").into())
            }
            ureq::Error::Status(code, resp) => Fail::Other(format!("{}: {code} {}", self.url, resp.into_string().unwrap_or_default().trim())),
            ureq::Error::Transport(t) => Fail::Other(format!("cannot reach the relay at {} ({t})", self.url)),
        }
    }

    fn json(&self, req: ureq::Request, body: Option<&Value>) -> Result<Value, Fail> {
        let resp = match body { Some(b) => req.send_json(b), None => req.call() }.map_err(|e| self.fail(e))?;
        if resp.status() == 204 { return Ok(Value::Null); }
        resp.into_json().map_err(|e| Fail::Other(format!("{}: unreadable answer: {e}", self.url)))
    }

    /// Puts the jelly on the relay, with the runtime its view runs on there.
    pub fn publish(&self, share: &str, doc: &Value, runtime: &str) -> Result<(), Fail> {
        self.json(self.at("PUT", share, ""), Some(&serde_json::json!({"doc": doc, "runtime": runtime}))).map(drop)
    }

    pub fn put_asset(&self, share: &str, file: &str, bytes: &[u8], content_type: &str) -> Result<(), Fail> {
        self.at("PUT", share, &format!("/assets/{}", encode(file))).set("Content-Type", content_type).send_bytes(bytes).map_err(|e| self.fail(e))?;
        Ok(())
    }

    pub fn remove_asset(&self, share: &str, file: &str) -> Result<(), Fail> {
        self.json(self.at("DELETE", share, &format!("/assets/{}", encode(file))), None).map(drop)
    }

    /// Lets the holder of `secret` in for `days`; returns when the share
    /// expires (`expires_at`, ms) and where the relay serves its pages.
    pub fn mint(&self, share: &str, secret: &str, who: &str, days: u64) -> Result<Value, Fail> {
        let body = serde_json::json!({"secret": secret, "who": who, "can_act": !who.is_empty(), "days": days});
        let resp = self.json(self.at("POST", share, "/people"), Some(&body))?;
        if resp["expires_at"].as_u64().is_none() {
            return Err(Fail::Other(format!("{}: unexpected answer {resp}", self.url)));
        }
        Ok(resp)
    }

    /// Puts the share in the room of the relay's PIN, making a new PIN when
    /// none is live; people can join at least `hours` more. The share lasts at
    /// least `days`. Returns the PIN (null when the room was open already),
    /// when it expires (ms), how many wrong tries lock it, and the room.
    pub fn pin(&self, share: &str, hours: u64, days: u64) -> Result<Value, Fail> {
        self.json(self.at("POST", share, "/pin"), Some(&serde_json::json!({"hours": hours, "days": days})))
    }

    /// Ends the PIN of the share's room, for every jelly in it; people who
    /// joined keep their links.
    pub fn unpin(&self, share: &str) -> Result<(), Fail> {
        self.json(self.at("DELETE", share, "/pin"), None).map(drop)
    }

    /// Stops every link of `who`'s, minted with `--to` or made by joining with
    /// the PIN; their open pages close at once.
    pub fn revoke(&self, share: &str, who: &str) -> Result<(), Fail> {
        self.json(self.at("DELETE", share, &format!("/people?who={}", encode(who))), None).map(drop)
    }

    /// Who holds a link, when each last opened one (`opened_ms`), how many of
    /// their pages are open now (`here`), and when the share expires.
    pub fn people(&self, share: &str) -> Result<Value, Fail> {
        self.json(self.at("GET", share, "/people"), None)
    }

    /// One change; returns the batch with its event.
    pub fn change(&self, share: &str, change: &crate::op::Change) -> Result<Value, Fail> {
        self.json(self.at("POST", share, "/op"), Some(&serde_json::to_value(change).expect("a change serializes")))
    }

    /// Ends the share; returns the events after `seq` it had not sent yet.
    pub fn end(&self, share: &str, seq: u64) -> Result<Value, Fail> {
        self.json(self.at("DELETE", share, &format!("?since={seq}")), None)
    }

    /// Takes a share down by its id, whether or not this home made it: its
    /// storage is deleted, every link stops and open pages close.
    pub fn takedown(&self, share: &str) -> Result<(), Fail> {
        self.at("DELETE", share, "").call().map_err(|e| self.fail(e))?;
        Ok(())
    }

    /// The daemon's own WebSocket to the share, catching up from `seq`.
    pub fn socket(&self, share: &str, seq: u64) -> String {
        format!("{}/api/live/{share}/ws?since={seq}", self.url.replacen("http", "ws", 1))
    }

    /// Where this relay serves the pages people open: the `share_origin` in
    /// its answer when it has one set (SHARE_ORIGIN), else its own URL.
    pub fn pages<'a>(&'a self, answer: &'a Value) -> &'a str {
        answer["share_origin"].as_str().unwrap_or(&self.url)
    }
}

/// A person's link, on the origin from `Relay::pages`.
pub fn link(pages: &str, share: &str, secret: &str) -> String {
    format!("{pages}/j/{share}{secret}")
}

/// Where people type a PIN.
pub fn join_page(pages: &str) -> String {
    format!("{pages}/pin")
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

    #[test]
    fn a_share_id_comes_from_a_link_or_stands_alone() {
        let (id, secret) = ("a1".repeat(11), "s".repeat(22));
        assert_eq!(share_of(&format!("https://pages.example/j/{id}{secret}")).unwrap(), id);
        assert_eq!(share_of(&format!("https://pages.example/j/{id}{secret}/")).unwrap(), id);
        assert_eq!(share_of(&id).unwrap(), id);
        assert!(share_of("https://pages.example/pin").is_err());
        assert!(share_of("plan").is_err());
    }
}
