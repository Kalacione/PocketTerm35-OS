//! Throughput history for the home's network graph. The menu is a new process
//! per open, so the daemon keeps the last few minutes for it.

use pt35_common::ipc::{Traffic, TRAFFIC_SAMPLES as KEEP};
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Default)]
pub struct History {
    iface: Option<String>,
    last: Option<(u64, u64, Instant)>,
    samples: VecDeque<[f32; 2]>,
}

impl History {
    /// Read the counters of `iface`, the interface carrying traffic now.
    pub fn sample(&mut self, iface: Option<&str>) {
        let counters = iface.and_then(bytes);
        self.push(iface, counters, Instant::now());
    }

    fn push(&mut self, iface: Option<&str>, counters: Option<(u64, u64)>, at: Instant) {
        if iface != self.iface.as_deref() {
            *self = History {
                iface: iface.map(str::to_string),
                ..History::default()
            };
        }
        let Some((down, up)) = counters else {
            self.last = None;
            return;
        };
        if let Some((last_down, last_up, last_at)) = self.last {
            let seconds = at.duration_since(last_at).as_secs_f64();
            if seconds > 0.0 {
                let bits =
                    |now: u64, then: u64| (now.saturating_sub(then) as f64 * 8.0 / seconds) as f32;
                if self.samples.len() == KEEP {
                    self.samples.pop_front();
                }
                self.samples
                    .push_back([bits(down, last_down), bits(up, last_up)]);
            }
        }
        self.last = Some((down, up, at));
    }

    pub fn snapshot(&self) -> Traffic {
        Traffic {
            iface: self.iface.clone(),
            samples: self.samples.iter().copied().collect(),
        }
    }
}

fn bytes(iface: &str) -> Option<(u64, u64)> {
    let read = |name: &str| -> Option<u64> {
        let path = format!("/sys/class/net/{iface}/statistics/{name}");
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    };
    Some((read("rx_bytes")?, read("tx_bytes")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rates_are_bits_a_second_and_restart_with_the_route() {
        let t0 = Instant::now();
        let mut h = History::default();
        h.push(Some("wlan0"), Some((0, 0)), t0);
        assert!(h.snapshot().samples.is_empty(), "one reading is no rate");
        h.push(
            Some("wlan0"),
            Some((2000, 500)),
            t0 + Duration::from_secs(2),
        );
        assert_eq!(h.snapshot().samples, vec![[8000.0, 2000.0]]);
        h.push(Some("eth0"), Some((9, 9)), t0 + Duration::from_secs(4));
        let s = h.snapshot();
        assert_eq!((s.iface.as_deref(), s.samples.len()), (Some("eth0"), 0));
    }

    #[test]
    fn keeps_three_minutes() {
        let t0 = Instant::now();
        let mut h = History::default();
        for i in 0..200u64 {
            h.push(Some("eth0"), Some((i, i)), t0 + Duration::from_secs(i * 2));
        }
        assert_eq!(h.snapshot().samples.len(), KEEP);
    }
}
