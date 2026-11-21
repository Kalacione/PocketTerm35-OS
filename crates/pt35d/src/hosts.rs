//! Whether the pinned hosts answer on their SSH port, and the wakes asked
//! for. Checked here, every couple of minutes, so opening the menu never
//! touches the network.

use pt35_common::ipc::{HostStatus, Wake};
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Gentle on the network: one connection per host every two minutes.
const EVERY: Duration = Duration::from_secs(120);
/// At boot the network comes up after pt35d. Until a host answers, the wait
/// doubles from here up to EVERY. An outage later keeps EVERY.
const RETRY: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(3);
/// A host reads DOWN only when a second check, this much later, agrees. At
/// boot the first check can beat the network or Tailscale.
const CONFIRM: Duration = Duration::from_secs(30);
/// A wake hook that has not ended by then is cut off.
const WAKE_LIMIT: Duration = Duration::from_secs(300);
/// How long a wake's outcome stays on its row, unless the host answers.
const WAKE_SHOWN: Duration = Duration::from_secs(600);

/// Each host's last reading, and how many checks in a row found it silent.
static CHECKED: Mutex<Vec<(HostStatus, u8)>> = Mutex::new(Vec::new());
static WAKES: Mutex<Vec<WakeRun>> = Mutex::new(Vec::new());

struct WakeRun {
    name: String,
    started: Instant,
    ended: Option<(Instant, Ended)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ended {
    Up,
    Failed(Option<i32>),
}

/// The pinned hosts, with what the last check found. A host pinned since
/// reads as not checked yet.
pub fn snapshot() -> Vec<HostStatus> {
    let checked = CHECKED.lock().map(|c| c.clone()).unwrap_or_default();
    let wakes = WAKES.lock();
    pt35_common::hosts::pinned()
        .into_iter()
        .take(pt35_common::hosts::SHOWN)
        .map(|name| {
            let mut host = checked
                .iter()
                .find(|(h, _)| h.name == name)
                .map(|(h, _)| h.clone())
                .unwrap_or(HostStatus {
                    name,
                    ..HostStatus::default()
                });
            if let Ok(wakes) = wakes.as_ref() {
                host.wake = wakes
                    .iter()
                    .find(|w| w.name == host.name)
                    .and_then(|w| shown(w, host.up));
            }
            host
        })
        .collect()
}

/// What a wake puts on its row. A host that answers needs none.
fn shown(run: &WakeRun, up: Option<bool>) -> Option<Wake> {
    match run.ended {
        None => Some(Wake::Waking(run.started.elapsed().as_secs() as u32)),
        Some((at, _)) if at.elapsed() > WAKE_SHOWN || up == Some(true) => None,
        Some((_, Ended::Up)) => Some(Wake::Away),
        Some((_, Ended::Failed(code))) => Some(Wake::Failed(code)),
    }
}

/// Run the wake hook for this host on the side. A host already waking is
/// left to it.
pub fn wake(name: &str) -> Result<(), String> {
    let hook =
        pt35_common::hosts::wake_hook().ok_or("no wake hook: ~/.config/pt35/hooks/hook_wake")?;
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("not a host: {name}"));
    }
    let mut wakes = WAKES.lock().map_err(|_| "wakes lock")?;
    if wakes.iter().any(|w| w.name == name && w.ended.is_none()) {
        return Ok(());
    }
    wakes.retain(|w| w.name != name);
    wakes.push(WakeRun {
        name: name.to_string(),
        started: Instant::now(),
        ended: None,
    });
    drop(wakes);
    let host_name = name.to_string();
    let spawned = std::thread::Builder::new()
        .name("wake".into())
        .spawn(move || {
            let ended = run_wake(&hook, &host_name);
            // Read the host before the outcome lands, so a host that is up
            // goes straight to UP and never shows as away.
            let name = host_name;
            if ended == Ended::Up {
                let host = check(&name);
                if host.up == Some(true) {
                    record(host);
                }
            }
            if let Ok(mut wakes) = WAKES.lock() {
                if let Some(run) = wakes.iter_mut().find(|w| w.name == name) {
                    run.ended = Some((Instant::now(), ended));
                }
            }
        });
    if let Err(e) = spawned {
        if let Ok(mut wakes) = WAKES.lock() {
            wakes.retain(|w| w.name != name);
        }
        return Err(format!("wake: {e}"));
    }
    Ok(())
}

/// The hook's exit, with the last line it wrote to stderr in the log.
fn run_wake(hook: &Path, host: &str) -> Ended {
    let mut child = match Command::new(hook)
        .arg(host)
        .env("PT35_HOST", host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::warn!("wake: {}: {e}", hook.display());
            return Ended::Failed(None);
        }
    };
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut out);
        }
        out
    });
    let deadline = Instant::now() + WAKE_LIMIT;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(500)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let said = reader.join().unwrap_or_default();
    let last = said
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    log::info!("wake {host}: exit {code:?} {last}");
    match code {
        Some(0) => Ended::Up,
        code => Ended::Failed(code),
    }
}

fn record(host: HostStatus) {
    if let Ok(mut checked) = CHECKED.lock() {
        match checked.iter_mut().find(|(h, _)| h.name == host.name) {
            Some(entry) => *entry = (host, 0),
            None => checked.push((host, 0)),
        }
    }
}

pub fn spawn() {
    let _ = std::thread::Builder::new().name("hosts".into()).spawn(|| {
        let mut last: Option<Instant> = None;
        let mut names: Vec<String> = Vec::new();
        let mut wait = Duration::ZERO;
        loop {
            let pins: Vec<String> = pt35_common::hosts::pinned()
                .into_iter()
                .take(pt35_common::hosts::SHOWN)
                .collect();
            // A new pin is checked at once, the rest on the clock.
            if pins != names || last.is_none_or(|at| at.elapsed() >= wait) {
                let before = CHECKED.lock().map(|c| c.clone()).unwrap_or_default();
                let results: Vec<(HostStatus, u8)> = pins
                    .iter()
                    .map(|n| confirm(before.iter().find(|(h, _)| &h.name == n), check(n)))
                    .collect();
                wait = if results.iter().any(|(_, fails)| *fails == 1) {
                    CONFIRM
                } else {
                    let hosts: Vec<HostStatus> = results.iter().map(|(h, _)| h.clone()).collect();
                    next_wait(wait, &hosts)
                };
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
        wake: None,
    }
}

/// One silent check is not enough to say DOWN: the host keeps what it read
/// before, and the next check, CONFIRM later, decides.
fn confirm(before: Option<&(HostStatus, u8)>, now: HostStatus) -> (HostStatus, u8) {
    if now.up == Some(true) {
        return (now, 0);
    }
    let fails = before.map_or(0, |(_, f)| *f).saturating_add(1);
    if fails >= 2 {
        return (now, fails);
    }
    let kept = before.map(|(h, _)| h.clone()).unwrap_or(HostStatus {
        name: now.name.clone(),
        address: now.address.clone(),
        ..HostStatus::default()
    });
    (kept, fails)
}

fn next_wait(wait: Duration, results: &[HostStatus]) -> Duration {
    if results.iter().any(|h| h.up == Some(true)) {
        EVERY
    } else {
        (wait * 2).clamp(RETRY, EVERY)
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

    #[test]
    fn down_takes_two_silent_checks() {
        let host = |up: Option<bool>| HostStatus {
            name: "nas".into(),
            up,
            ..HostStatus::default()
        };
        let first = confirm(None, host(Some(false)));
        assert_eq!(first, (host(None), 1));
        let second = confirm(Some(&first), host(Some(false)));
        assert_eq!(second, (host(Some(false)), 2));
        let was_up = (host(Some(true)), 0);
        assert_eq!(
            confirm(Some(&was_up), host(Some(false))),
            (host(Some(true)), 1)
        );
        assert_eq!(
            confirm(Some(&second), host(Some(true))),
            (host(Some(true)), 0)
        );
    }

    #[test]
    fn a_wake_shows_until_the_host_answers() {
        let run = |ended| WakeRun {
            name: "nas".into(),
            started: Instant::now(),
            ended,
        };
        assert_eq!(shown(&run(None), Some(false)), Some(Wake::Waking(0)));
        let now = Instant::now();
        assert_eq!(
            shown(&run(Some((now, Ended::Up))), Some(false)),
            Some(Wake::Away)
        );
        assert_eq!(shown(&run(Some((now, Ended::Up))), Some(true)), None);
        assert_eq!(
            shown(&run(Some((now, Ended::Failed(Some(1))))), Some(false)),
            Some(Wake::Failed(Some(1)))
        );
    }

    #[test]
    fn a_wake_reports_its_exit() {
        use std::os::unix::fs::PermissionsExt;
        let sh = |script: &str| {
            let path = std::env::temp_dir().join(format!("pt35-wake-{}", std::process::id()));
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            let ended = run_wake(&path, "nas");
            std::fs::remove_file(&path).unwrap();
            ended
        };
        assert_eq!(sh(r#"[ "$1" = nas ] && [ "$PT35_HOST" = nas ]"#), Ended::Up);
        assert_eq!(sh("exit 2"), Ended::Failed(Some(2)));
        assert_eq!(
            run_wake(Path::new("/nonexistent/hook_wake"), "nas"),
            Ended::Failed(None)
        );
    }

    #[test]
    fn retries_sooner_while_all_down() {
        let host = |up| HostStatus {
            up: Some(up),
            ..HostStatus::default()
        };
        assert_eq!(next_wait(Duration::ZERO, &[host(false)]), RETRY);
        assert_eq!(next_wait(RETRY, &[host(false)]), RETRY * 2);
        assert_eq!(next_wait(EVERY, &[host(false)]), EVERY);
        assert_eq!(next_wait(RETRY, &[host(false), host(true)]), EVERY);
    }
}
