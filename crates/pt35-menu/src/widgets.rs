//! What the widget column of the launcher shows: the machine, the network,
//! and the media player when one is playing.
//! Read on the menu's poll, never before the first frame.

use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(Default)]
pub struct Readings {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub temperature: Option<f32>,
    pub address: Option<String>,
    pub ssid: Option<String>,
    /// Bytes a second, down and up, on the interface carrying traffic.
    pub rates: Option<(f64, f64)>,
    sampled: bool,
    cpu_last: Option<(u64, u64)>,
    net_last: Option<(String, u64, u64, Instant)>,
    /// The address and the network name cost a process each: read once, off
    /// the Wayland thread.
    slow: Option<Receiver<(Option<String>, Option<String>)>>,
    /// The player, over MPRIS. Some while one is playing or paused.
    pub media: Option<Media>,
    media_rx: Option<Receiver<Option<Media>>>,
}

/// What an MPRIS player is playing, as `playerctl` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub playing: bool,
    pub title: String,
    pub artist: Option<String>,
    /// Seconds.
    pub position: Option<f64>,
    pub length: Option<f64>,
}

const MEDIA_FORMAT: &str =
    "{{status}}\t{{title}}\t{{artist}}\t{{position}}\t{{mpris:length}}\t{{xesam:url}}";

impl Readings {
    pub fn sampled(&self) -> bool {
        self.sampled
    }

    /// Read everything again. `iface` is the one pt35d says carries traffic.
    pub fn sample(&mut self, iface: Option<&str>) {
        if !self.sampled {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send((crate::providers::ip_address(), ssid()));
            });
            self.slow = Some(rx);
        }
        self.sampled = true;
        // playerctl is a process per read: off the Wayland thread, one at a time.
        match self.media_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(media)) => {
                self.media = media;
                self.media_rx = None;
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {}
            _ => {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let out =
                        crate::providers::run("playerctl", &["metadata", "--format", MEDIA_FORMAT]);
                    let _ = tx.send(out.as_deref().and_then(parse_media));
                });
                self.media_rx = Some(rx);
            }
        }
        if let Some((address, ssid)) = self.slow.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.address, self.ssid) = (address, ssid);
            self.slow = None;
        }
        let now = cpu_times();
        self.cpu = match (self.cpu_last, now) {
            (Some(last), Some(now)) => cpu_percent(last, now),
            _ => None,
        };
        self.cpu_last = now;
        self.memory = crate::providers::memory()
            .map(|(used, total)| (used * 100 / total.max(1)).min(100) as u8);
        self.temperature = crate::providers::temperature();
        self.rates = None;
        let Some(iface) = iface else {
            self.net_last = None;
            return;
        };
        let Some((down, up)) = net_bytes(iface) else {
            return;
        };
        let at = Instant::now();
        if let Some((last_iface, last_down, last_up, last_at)) = &self.net_last {
            let seconds = at.duration_since(*last_at).as_secs_f64();
            if last_iface == iface && seconds > 0.0 {
                self.rates = Some((
                    down.saturating_sub(*last_down) as f64 / seconds,
                    up.saturating_sub(*last_up) as f64 / seconds,
                ));
            }
        }
        self.net_last = Some((iface.to_string(), down, up, at));
    }
}

impl Readings {
    /// Play or pause, and show it at once rather than on the next read.
    pub fn play_pause(&mut self) {
        let Some(media) = self.media.as_mut() else {
            return;
        };
        media.playing = !media.playing;
        if let Ok(mut child) = std::process::Command::new("playerctl")
            .arg("play-pause")
            .spawn()
        {
            std::thread::spawn(move || child.wait());
        }
    }
}

/// One line of `MEDIA_FORMAT`. A stopped player, or none, is no widget.
fn parse_media(out: &str) -> Option<Media> {
    let mut f = out.trim_end_matches('\n').split('\t');
    let playing = match f.next()? {
        "Playing" => true,
        "Paused" => false,
        _ => return None,
    };
    let text = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let micros = |s: Option<&str>| {
        s.and_then(|s| s.trim().parse::<f64>().ok())
            .filter(|m| *m > 0.0)
            .map(|m| m / 1e6)
    };
    let title = text(f.next());
    let artist = text(f.next());
    let position = micros(f.next());
    let length = micros(f.next());
    // A file with no tags: its name, as the player shows it.
    let title = title
        .or_else(|| text(f.next()).and_then(|url| url.rsplit('/').next().map(String::from)))
        .unwrap_or_else(|| "Unknown".into());
    Some(Media {
        playing,
        title,
        artist,
        position: position.or(length.map(|_| 0.0)),
        length,
    })
}

/// `1:23`, `1:02:03`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match s / 3600 {
        0 => format!("{}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// Idle and total jiffies, from the first line of /proc/stat.
fn cpu_times() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    parse_cpu(text.lines().next()?)
}

fn parse_cpu(line: &str) -> Option<(u64, u64)> {
    let fields: Vec<u64> = line
        .strip_prefix("cpu ")?
        .split_whitespace()
        .filter_map(|f| f.parse().ok())
        .collect();
    // idle plus iowait: waiting on the disk is not work.
    let idle = fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    Some((idle, fields.iter().sum()))
}

pub fn cpu_percent(last: (u64, u64), now: (u64, u64)) -> Option<u8> {
    let total = now.1.checked_sub(last.1).filter(|t| *t > 0)?;
    let idle = now.0.saturating_sub(last.0).min(total);
    Some(((total - idle) * 100 / total) as u8)
}

fn net_bytes(iface: &str) -> Option<(u64, u64)> {
    let read = |name: &str| -> Option<u64> {
        let path = format!("/sys/class/net/{iface}/statistics/{name}");
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    };
    Some((read("rx_bytes")?, read("tx_bytes")?))
}

/// The Wi-Fi connection's name, from NetworkManager's active list: no scan.
fn ssid() -> Option<String> {
    let out = crate::providers::run(
        "nmcli",
        &["-t", "-f", "NAME,TYPE", "connection", "show", "--active"],
    )?;
    out.lines()
        .find_map(|line| line.strip_suffix(":802-11-wireless"))
        .map(|name| name.replace("\\:", ":"))
}

/// `2.4 MB/s`, `320 kB/s`, `0 kB/s`.
pub fn rate(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1_000_000.0 {
        format!("{:.1} MB/s", bytes_per_second / 1_000_000.0)
    } else {
        format!("{:.0} kB/s", bytes_per_second / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_load_is_the_share_of_time_not_idle() {
        let last = parse_cpu("cpu  100 0 100 700 100 0 0 0 0 0").unwrap();
        let now = parse_cpu("cpu  150 0 150 850 150 0 0 0 0 0").unwrap();
        assert_eq!(last, (800, 1000));
        assert_eq!(cpu_percent(last, now), Some(33));
        assert_eq!(cpu_percent(now, now), None, "no time passed");
    }

    #[test]
    fn media_reads_playerctl() {
        let m = parse_media("Playing\tSong\tBand\t83000000\t296000000\tfile:///m/a.mp3\n").unwrap();
        assert!(m.playing);
        assert_eq!(
            (m.title.as_str(), m.artist.as_deref()),
            ("Song", Some("Band"))
        );
        assert_eq!((m.position, m.length), (Some(83.0), Some(296.0)));
        let m = parse_media("Paused\t\t\t\t\tfile:///m/a%20b.mp3").unwrap();
        assert!(!m.playing);
        assert_eq!(m.title, "a%20b.mp3");
        assert_eq!((m.artist, m.length), (None, None));
        assert_eq!(parse_media("Stopped\tSong\t\t\t\t"), None);
        assert_eq!(parse_media(""), None);
    }

    #[test]
    fn clocks_read_like_a_player() {
        assert_eq!(clock(83.4), "1:23");
        assert_eq!(clock(3723.0), "1:02:03");
    }

    #[test]
    fn rates_read_at_a_glance() {
        assert_eq!(rate(2_400_000.0), "2.4 MB/s");
        assert_eq!(rate(320_000.0), "320 kB/s");
        assert_eq!(rate(0.0), "0 kB/s");
    }
}
