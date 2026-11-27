//! The one address besides this machine's own that the daemon trusts
//! (`jelly trust`), kept as an origin in `<home>/trust.json`. The daemon still
//! listens on 127.0.0.1 only: the person puts a proxy they trust in front of it,
//! such as `tailscale serve`, and that proxy decides who gets in. Only the CLI
//! writes the file, so nothing that reaches the port can open the daemon to
//! another address.

use std::path::{Path, PathBuf};

fn file(home: &Path) -> PathBuf {
    home.join("trust.json")
}

/// The origin saved in this home, such as `https://mini.example.ts.net`.
pub fn read(home: &Path) -> Result<Option<String>, String> {
    let path = file(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Saves `address` as the origin the daemon trusts, and returns it as
/// saved. An empty address removes it.
pub fn set(home: &Path, address: &str) -> Result<Option<String>, String> {
    let path = file(home);
    let fail = |e: std::io::Error| format!("{}: {e}", path.display());
    if address.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(fail(e)),
            _ => Ok(None),
        };
    }
    let origin = origin(address)?;
    std::fs::write(&path, serde_json::to_string(&origin).expect("serializes") + "\n").map_err(fail)?;
    Ok(Some(origin))
}

/// `address` as a browser writes an origin: scheme, host, and the port when it
/// is not the scheme's own.
fn origin(address: &str) -> Result<String, String> {
    let url = url::Url::parse(address).map_err(|e| format!("{address}: {e}; give an address such as https://mini.example.ts.net"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{address}: the address must start with http:// or https://"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{address}: give the address alone, with no path, query or login"));
    }
    let local = match url.host() {
        Some(url::Host::Domain(name)) => matches!(name, "localhost" | "localhost."),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => std::net::IpAddr::V6(ip).to_canonical().is_loopback(),
        None => return Err(format!("{address}: the address has no host")),
    };
    if local {
        return Err(format!("{address} is this machine, where jelly already answers"));
    }
    Ok(url.origin().ascii_serialization())
}

/// Whether a request with this `Host`, and this `Origin` when a browser sent
/// one, came through the saved origin.
pub fn allows(trusted: &str, host: &str, origin: Option<&str>) -> bool {
    let authority = trusted.split_once("://").map_or(trusted, |(_, a)| a);
    host.eq_ignore_ascii_case(authority) && origin.is_none_or(|o| o.eq_ignore_ascii_case(trusted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_saved_as_its_origin_and_an_empty_one_removes_it() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read(home.path()).unwrap(), None);
        assert_eq!(set(home.path(), "HTTPS://Mini.Example.ts.net:443/").unwrap().as_deref(), Some("https://mini.example.ts.net"));
        assert_eq!(read(home.path()).unwrap().as_deref(), Some("https://mini.example.ts.net"));
        assert_eq!(set(home.path(), "http://mini:8080").unwrap().as_deref(), Some("http://mini:8080"));
        assert_eq!(set(home.path(), "").unwrap(), None);
        assert_eq!(read(home.path()).unwrap(), None);
        assert_eq!(set(home.path(), "").unwrap(), None);
    }

    #[test]
    fn an_address_that_is_not_one_other_origin_is_refused() {
        let home = tempfile::tempdir().unwrap();
        for bad in ["mini.example.ts.net", "ftp://mini", "https://mini/jellies", "https://mini?x=1", "https://me@mini", "http://localhost:7878", "http://127.0.0.1:7878", "http://[::1]:7878", "http://[::ffff:127.0.0.1]"] {
            assert!(set(home.path(), bad).is_err(), "{bad}");
        }
        assert_eq!(read(home.path()).unwrap(), None);
    }

    #[test]
    fn only_the_saved_origin_passes() {
        let trusted = "https://mini.example.ts.net";
        assert!(allows(trusted, "mini.example.ts.net", None));
        assert!(allows(trusted, "Mini.Example.ts.net", Some("https://mini.example.ts.net")));
        assert!(!allows(trusted, "mini.example.ts.net", Some("http://mini.example.ts.net")));
        assert!(!allows(trusted, "mini.example.ts.net", Some("https://evil.example")));
        assert!(!allows(trusted, "mini.example.ts.net:8443", None));
        assert!(!allows(trusted, "evil.example", None));
        assert!(allows("http://mini:8080", "mini:8080", Some("http://mini:8080")));
        assert!(!allows("http://mini:8080", "mini", None));
    }
}
