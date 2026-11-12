use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_URL: &str = "http://127.0.0.1:7878";

pub fn ensure_running(url: &str, quiet: bool, command: Command) -> Result<(), String> {
    ensure_with_timeout(url, quiet, command, Duration::from_secs(10))
}

pub fn is_default_endpoint(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let local = match url.host() {
        Some(url::Host::Domain("localhost" | "localhost.")) => true,
        Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(ip)) => ip.to_ipv4_mapped() == Some(std::net::Ipv4Addr::LOCALHOST),
        _ => false,
    };
    let default = url::Url::parse(DEFAULT_URL).expect("valid default URL");
    local && url.scheme() == default.scheme()
        && url.port_or_known_default() == default.port_or_known_default()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

enum Probe {
    Ready,
    Waiting { refused: bool, error: String },
    Failed(String),
}

fn probe(url: &str, timeout: Duration) -> Probe {
    match ureq::get(&format!("{}/api/jellies", url.trim_end_matches('/')))
        .timeout(timeout)
        .call()
    {
        Ok(response) => match response.into_json::<serde_json::Value>() {
            Ok(value) if value.is_array() => Probe::Ready,
            Ok(_) => Probe::Failed("expected a JSON array from /api/jellies".into()),
            Err(e) => Probe::Failed(e.to_string()),
        },
        Err(ureq::Error::Transport(t)) => Probe::Waiting {
            refused: t.kind() == ureq::ErrorKind::ConnectionFailed,
            error: t.to_string(),
        },
        Err(e) => Probe::Failed(e.to_string()),
    }
}

pub(super) fn ensure_with_timeout(
    url: &str,
    quiet: bool,
    command: Command,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let request_timeout = || {
        deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(250))
    };
    let first = probe(url, request_timeout());
    if matches!(first, Probe::Ready) {
        return Ok(());
    }
    let should_spawn = matches!(&first, Probe::Waiting { refused: true, .. });
    let mut child = if should_spawn {
        Some(spawn_detached(command).map_err(|e| {
            format!("cannot start daemon at {url}: {e}; run `jelly serve` for details")
        })?)
    } else {
        None
    };
    let mut exit = None;
    let mut last = first;
    loop {
        if let Some(candidate) = child.as_mut() {
            if let Ok(Some(status)) = candidate.try_wait() {
                exit = Some(status);
                child = None;
            }
        }
        if matches!(last, Probe::Ready) {
            if should_spawn && !quiet {
                eprintln!("jelly: daemon started at {url}");
            }
            return Ok(());
        }
        // HTTP and malformed responses identify a different or broken service.
        let retry = matches!(&last, Probe::Waiting { .. });
        if !retry || Instant::now() >= deadline {
            let problem = match last {
                Probe::Waiting { error, .. } | Probe::Failed(error) => error,
                Probe::Ready => unreachable!(),
            };
            return Err(format!(
                "daemon at {url} did not become ready: {problem}{}; run `jelly serve` for details",
                exit.map(|s| format!(" (startup process {s})"))
                    .unwrap_or_default()
            ));
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
        if Instant::now() < deadline {
            last = probe(url, request_timeout());
        }
    }
}

pub fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pre_exec must remain async-signal-safe after fork in a threaded client.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::is_default_endpoint;

    #[test]
    fn default_endpoint_aliases_and_other_targets() {
        for url in [
            "http://127.0.0.1:7878", "http://127.0.0.1:7878/",
            "HTTP://LOCALHOST:7878/", "http://localhost.:7878",
            "http://127.1:7878", "http://2130706433:7878",
            "http://[::ffff:127.0.0.1]:7878",
        ] {
            assert!(is_default_endpoint(url), "{url}");
        }
        for url in [
            "http://127.0.0.1:7879", "https://localhost:7878", "http://localhost",
            "http://127.0.0.2:7878", "http://[::1]:7878", "http://example.com:7878",
            "http://localhost:7878/prefix", "http://localhost:7878?x=1",
            "http://localhost:7878#fragment", "http://user@localhost:7878", "invalid",
        ] {
            assert!(!is_default_endpoint(url), "{url}");
        }
    }
}
