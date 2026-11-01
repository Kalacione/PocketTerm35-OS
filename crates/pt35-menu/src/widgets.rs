//! What the widget column of the launcher shows: the machine on top, then a
//! pile of pinned widgets, one at a time: the network and the media player.
//! Read on the menu's poll, never before the first frame.

use std::sync::mpsc::Receiver;

#[derive(Default)]
pub struct Readings {
    pub cpu: Option<u8>,
    pub memory: Option<u8>,
    pub temperature: Option<f32>,
    pub address: Option<String>,
    pub ssid: Option<String>,
    /// The last minutes of throughput, from pt35d.
    pub traffic: Option<pt35_common::ipc::Traffic>,
    sampled: bool,
    cpu_last: Option<(u64, u64)>,
    /// The address and the network name cost a process each: read once, off
    /// the Wayland thread.
    slow: Option<Receiver<Slow>>,
    /// The SD card and any USB drive, read once per open. `None` until read.
    pub volumes: Option<Vec<Volume>>,
    /// The pinned hosts as pt35d last found them.
    pub hosts: Vec<pt35_common::ipc::HostStatus>,
    /// The player, over MPRIS. Some while one is playing or paused.
    pub media: Option<Media>,
    media_rx: Option<Receiver<Option<Media>>>,
}

/// Address, network name and volumes: a process each, read once per open.
type Slow = (Option<String>, Option<String>, Option<Vec<Volume>>);

/// A mounted drive, as `df` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    /// `SD`, `NVME` or `USB`, from the device.
    pub kind: &'static str,
    /// `/` for the system, else the mount point's last part.
    pub name: String,
    pub size: u64,
    pub free: u64,
}

impl Volume {
    pub fn used(&self) -> f32 {
        1.0 - self.free as f32 / self.size.max(1) as f32
    }
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

    /// Read everything again.
    pub fn sample(&mut self) {
        if !self.sampled {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send((crate::providers::ip_address(), ssid(), volumes()));
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
        if let Some((address, ssid, volumes)) = self.slow.as_ref().and_then(|rx| rx.try_recv().ok())
        {
            (self.address, self.ssid, self.volumes) = (address, ssid, volumes);
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
        self.traffic = match crate::live::request(&pt35_common::ipc::Request::Traffic) {
            Ok(pt35_common::ipc::Response::Traffic(traffic)) => Some(traffic),
            _ => None,
        };
        if let Ok(pt35_common::ipc::Response::Hosts { hosts }) =
            crate::live::request(&pt35_common::ipc::Request::Hosts)
        {
            self.hosts = hosts;
        }
    }
}

impl Readings {
    /// `play-pause`, `previous` or `next`. A pause shows at once rather than
    /// on the next read.
    pub fn control(&mut self, command: &str) {
        let Some(media) = self.media.as_mut() else {
            return;
        };
        if command == "play-pause" {
            media.playing = !media.playing;
        }
        if let Ok(mut child) = std::process::Command::new("playerctl").arg(command).spawn() {
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

fn volumes() -> Option<Vec<Volume>> {
    crate::providers::run("df", &["-B1", "--output=source,size,avail,target"])
        .map(|out| parse_df(&out))
}

/// The system and removable drives. Not tmpfs, not boot partitions.
fn parse_df(out: &str) -> Vec<Volume> {
    let mut out: Vec<Volume> = out
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let (source, size, free) = (f.next()?, f.next()?, f.next()?);
            let target = f.collect::<Vec<_>>().join(" ");
            let removable = ["/media/", "/run/media/", "/mnt/"]
                .iter()
                .any(|p| target.starts_with(p));
            if !source.starts_with("/dev/") || !(target == "/" || removable) {
                return None;
            }
            let kind = if source.contains("mmcblk") {
                "SD"
            } else if source.contains("nvme") {
                "NVME"
            } else {
                "USB"
            };
            let name = match target.as_str() {
                "/" => "/".to_string(),
                t => t.rsplit('/').next().unwrap_or(t).to_uppercase(),
            };
            Some(Volume {
                kind,
                name,
                size: size.parse().ok()?,
                free: free.parse().ok()?,
            })
        })
        .collect();
    // The system first.
    out.sort_by_key(|v| v.name != "/");
    out
}

/// `72 GB`, `1.2 GB`, `640 MB`.
pub fn bytes(n: u64) -> String {
    match n as f64 {
        b if b >= 10e9 => format!("{:.0} GB", b / 1e9),
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b => format!("{:.0} MB", b / 1e6),
    }
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

/// In bits, as links and plans are sold: `2.4 Mb/s`, `320 kb/s`, `0 kb/s`.
pub fn rate(bits_per_second: f32) -> String {
    match bits_per_second {
        b if b >= 1e9 => format!("{:.1} Gb/s", b / 1e9),
        b if b >= 1e6 => format!("{:.1} Mb/s", b / 1e6),
        b => format!("{:.0} kb/s", b / 1e3),
    }
}

/// Where a rate sits on the graph, 0 to 1: a log scale from 1 kb/s to 1 Gb/s,
/// so a quiet link and a download read on the same graph. Its lines are the
/// units: k at the floor, M halfway, G at the top.
pub fn log_level(bits_per_second: f32) -> f32 {
    ((bits_per_second.max(1e3).log10() - 3.0) / 6.0).clamp(0.0, 1.0)
}

/// What the pile at the bottom of the widget column can hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Widget {
    Network,
    Media,
    Storage,
    Hosts,
}

impl Widget {
    pub const ALL: [Widget; 4] = [
        Widget::Network,
        Widget::Media,
        Widget::Storage,
        Widget::Hosts,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Widget::Network => "network",
            Widget::Media => "media",
            Widget::Storage => "storage",
            Widget::Hosts => "hosts",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Widget::Network => "Network",
            Widget::Media => "Media",
            Widget::Storage => "Storage",
            Widget::Hosts => "Hosts",
        }
    }

    pub fn from_id(id: &str) -> Option<Widget> {
        Widget::ALL.into_iter().find(|w| w.id() == id)
    }
}

/// The pinned widgets, in order. Never set is all of them.
pub fn pinned() -> Vec<Widget> {
    match std::fs::read_to_string(pt35_common::paths::widgets_path()) {
        Ok(text) => parse_widgets(&text),
        Err(_) => Widget::ALL.to_vec(),
    }
}

fn parse_widgets(text: &str) -> Vec<Widget> {
    let mut out = Vec::new();
    for w in text.lines().filter_map(|l| Widget::from_id(l.trim())) {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// Pin a widget, or unpin it. Returns whether it is pinned now.
pub fn toggle_pin(widget: Widget) -> std::io::Result<bool> {
    let mut list = pinned();
    let pinned = match list.iter().position(|w| *w == widget) {
        Some(at) => {
            list.remove(at);
            false
        }
        None => {
            list.push(widget);
            true
        }
    };
    let text: String = list.iter().map(|w| format!("{}\n", w.id())).collect();
    pt35_common::write_atomic(&pt35_common::paths::widgets_path(), text.as_bytes())?;
    Ok(pinned)
}

/// The pinned widgets and where the pile stands in them: on the one the
/// home showed last, so the next open shows it again.
pub fn pile() -> (Vec<Widget>, usize) {
    let list = pinned();
    let at = std::fs::read_to_string(pt35_common::paths::widget_path())
        .ok()
        .and_then(|text| Widget::from_id(text.trim()))
        .and_then(|shown| list.iter().position(|w| *w == shown))
        .unwrap_or(0);
    (list, at)
}

pub fn set_shown(widget: Widget) {
    let _ = pt35_common::write_atomic(
        &pt35_common::paths::widget_path(),
        format!("{}\n", widget.id()).as_bytes(),
    );
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
        assert_eq!(rate(2_400_000.0), "2.4 Mb/s");
        assert_eq!(rate(320_000.0), "320 kb/s");
        assert_eq!(rate(0.0), "0 kb/s");
        assert_eq!(rate(1.2e9), "1.2 Gb/s");
    }

    #[test]
    fn the_graph_is_a_log_from_k_to_g() {
        assert_eq!(log_level(0.0), 0.0);
        assert_eq!(log_level(1e3), 0.0);
        assert!((log_level(1e6) - 0.5).abs() < 1e-6);
        assert_eq!(log_level(1e9), 1.0);
        assert_eq!(log_level(1e11), 1.0);
    }

    #[test]
    fn volumes_are_the_system_and_removable_drives() {
        let df = "Filesystem Size Avail Mounted on
/dev/sda1 64000000000 1000000000 /media/dje/MY KEY
tmpfs 100 50 /run
/dev/mmcblk0p2 120000000000 72000000000 /
/dev/mmcblk0p1 500000000 400000000 /boot/firmware
";
        let v = parse_df(df);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].kind, v[0].name.as_str()), ("SD", "/"));
        assert_eq!((v[1].kind, v[1].name.as_str()), ("USB", "MY KEY"));
        assert_eq!(bytes(v[0].free), "72 GB");
        assert_eq!(bytes(v[1].free), "1.0 GB");
        assert_eq!(bytes(640_000_000), "640 MB");
    }

    #[test]
    fn widget_pins_keep_their_order_and_skip_unknowns() {
        assert_eq!(
            parse_widgets("media\nclock\nnetwork\nmedia\n"),
            vec![Widget::Media, Widget::Network]
        );
        assert_eq!(parse_widgets(""), vec![]);
    }
}
