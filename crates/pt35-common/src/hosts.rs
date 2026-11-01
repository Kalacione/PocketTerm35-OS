//! The machines in `~/.ssh/config`, and the ones pinned to the home's HOSTS
//! widget. The list stays on the device: nothing about it is in a repository.

use std::path::{Path, PathBuf};

/// How many hosts the widget has room for.
pub const SHOWN: usize = 3;

fn ssh_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".ssh")
}

/// Every concrete `Host` in `~/.ssh/config` and the files it includes, in
/// order. Patterns (`*`, `?`, `!`) are rules, not machines.
pub fn ssh_hosts() -> Vec<String> {
    hosts_in(&ssh_dir())
}

fn hosts_in(ssh: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(ssh, &ssh.join("config"), &mut out, 0);
    out
}

fn collect(ssh: &Path, path: &Path, out: &mut Vec<String>, depth: u8) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for (keyword, args) in text.lines().filter_map(directive) {
        if keyword.eq_ignore_ascii_case("host") {
            for name in args.split_whitespace() {
                if !name.contains(['*', '?', '!']) && !out.iter().any(|seen| seen == name) {
                    out.push(name.to_string());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < 3 {
            for pattern in args.split_whitespace() {
                for file in expand(ssh, pattern) {
                    collect(ssh, &file, out, depth + 1);
                }
            }
        }
    }
}

/// `Keyword args` or `Keyword=args`, comments and blanks dropped.
fn directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let at = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let args = line[at..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    Some((&line[..at], args))
}

/// An `Include` path: relative to `~/.ssh`, with a `*` allowed in its last
/// part, as in `config.d/*`.
fn expand(ssh: &Path, pattern: &str) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => ssh.parent().unwrap_or(Path::new("/")).join(rest),
        None if pattern.starts_with('/') => PathBuf::from(pattern),
        None => ssh.join(pattern),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some((before, after)) = name.split_once('*') else {
        return vec![path];
    };
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy())
                .is_some_and(|n| n.starts_with(before) && n.ends_with(after))
        })
        .collect();
    files.sort();
    files
}

/// The pinned hosts, in order. Never set is the first few of the config.
pub fn pinned() -> Vec<String> {
    match std::fs::read_to_string(crate::paths::hosts_path()) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => ssh_hosts().into_iter().take(SHOWN).collect(),
    }
}

/// Pin a host to the widget, or unpin it. Returns whether it is pinned now.
pub fn toggle_pin(name: &str) -> std::io::Result<bool> {
    let mut list = pinned();
    let pinned = match list.iter().position(|h| h == name) {
        Some(at) => {
            list.remove(at);
            false
        }
        None => {
            list.push(name.to_string());
            true
        }
    };
    let text: String = list.iter().map(|h| format!("{h}\n")).collect();
    crate::write_atomic(&crate::paths::hosts_path(), text.as_bytes())?;
    Ok(pinned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hosts_and_skips_patterns() {
        let dir = std::env::temp_dir().join(format!("pt35-ssh-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".ssh/config.d")).unwrap();
        std::fs::write(
            dir.join(".ssh/config"),
            "# mine\nHost nas backup\n  HostName 10.0.0.2\nHost *.lan !x\nInclude config.d/*\nhost=jm\n",
        )
        .unwrap();
        std::fs::write(dir.join(".ssh/config.d/a.conf"), "Host ha\nHost nas\n").unwrap();
        assert_eq!(
            hosts_in(&dir.join(".ssh")),
            vec!["nas", "backup", "ha", "jm"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn directives_take_either_form() {
        assert_eq!(directive("  Host  nas"), Some(("Host", "nas")));
        assert_eq!(directive("Host=nas"), Some(("Host", "nas")));
        assert_eq!(directive("# Host nas"), None);
    }
}
