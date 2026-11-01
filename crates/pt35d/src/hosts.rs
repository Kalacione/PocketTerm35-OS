//! Whether the pinned hosts answer on their SSH port. Checked here, every
//! couple of minutes, so opening the menu never touches the network.

use pt35_common::ipc::HostStatus;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Gentle on the network: one connection per host every two minutes.
const EVERY: Duration = Duration::from_secs(120);
const TIMEOUT: Duration = Duration::from_secs(3);

static CHECKED: Mutex<Vec<HostStatus>> = Mutex::new(Vec::new());

/// The pinned hosts, with what the last check found. A host pinned since
/// reads as not checked yet.
pub fn snapshot() -> Vec<HostStatus> {
    let checked = CHECKED.lock().map(|c| c.clone()).unwrap_or_default();
    pt35_common::hosts::pinned()
        .into_iter()
        .take(pt35_common::hosts::SHOWN)
        .map(|name| {
            checked
                .iter()
                .find(|h| h.name == name)
                .cloned()
                .unwrap_or(HostStatus {
                    name,
                    ..HostStatus::default()
                })
        })
        .collect()
}

pub fn spawn() {
    let _ = std::thread::Builder::new().name("hosts".into()).spawn(|| {
        let mut last: Option<Instant> = None;
        let mut names: Vec<String> = Vec::new();
        loop {
            let pins: Vec<String> = pt35_common::hosts::pinned()
                .into_iter()
                .take(pt35_common::hosts::SHOWN)
                .collect();
            // A new pin is checked at once, the rest on the clock.
            if pins != names || last.is_none_or(|at| at.elapsed() >= EVERY) {
                let results: Vec<HostStatus> = pins.iter().map(|n| check(n)).collect();
                if let Ok(mut checked) = CHECKED.lock() {
                    *checked = results;
                }
                names = pins;
                last = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Resolve the host as ssh does (`ssh -G`: HostName, Port, Include, Match),
/// then time a TCP connection to its port.
fn check(name: &str) -> HostStatus {
    let resolved = pt35_common::proc::output_within(
        std::process::Command::new("ssh").args(["-G", "--", name]),
        TIMEOUT,
    );
    let (address, port) = resolved.as_deref().map(parse_g).unwrap_or_default();
    let address = address.unwrap_or_else(|| name.to_string());
    let at = Instant::now();
    let up = (address.as_str(), port.unwrap_or(22))
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .is_some_and(|addr| TcpStream::connect_timeout(&addr, TIMEOUT).is_ok());
    HostStatus {
        name: name.to_string(),
        address: Some(address),
        up: Some(up),
        latency_ms: up.then(|| at.elapsed().as_millis().min(u32::MAX as u128) as u32),
    }
}

fn parse_g(out: &str) -> (Option<String>, Option<u16>) {
    let value = |key: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(' '))
            .map(str::trim)
    };
    (
        value("hostname").map(str::to_string),
        value("port").and_then(|p| p.parse().ok()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_ssh_resolved() {
        let out = "user dje\nhostname 10.0.0.2\nport 2222\nhostkeyalias x\n";
        assert_eq!(parse_g(out), (Some("10.0.0.2".into()), Some(2222)));
        assert_eq!(parse_g(""), (None, None));
    }
}
