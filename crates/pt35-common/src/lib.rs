//! Shared building blocks for the pt35-desktop shell.
//!
//! Everything in here is plain data: configuration loading (system defaults
//! merged with user overrides), the theme, the menu tree, application profiles
//! and the `pt35d` IPC protocol. No Wayland, no Linux-only syscalls — so it
//! builds and tests on any host.

pub mod apps;
pub mod hosts;
pub mod ipc;
pub mod menu;
pub mod paths;
pub mod proc;
pub mod sway;
pub mod theme;

mod merge;

pub use merge::merge_toml;

/// Load a TOML document from the system defaults, overlaid with the user's copy.
///
/// Both files are optional: missing files fall back to `T::default()` via serde.
pub fn load_config<T: serde::de::DeserializeOwned + Default>(relative: &str) -> T {
    let system = paths::system_config(relative);
    let user = paths::user_config(relative);
    load_config_from(system.as_deref(), user.as_deref())
}

/// Same as [`load_config`], with explicit paths (used by tests and `--config`).
pub fn load_config_from<T: serde::de::DeserializeOwned + Default>(
    system: Option<&std::path::Path>,
    user: Option<&std::path::Path>,
) -> T {
    logged(load_layers(&[system, user]))
}

/// The theme, in three layers: the shipped defaults, what was picked in
/// Settings > Appearance, then the user's own `theme.toml`.
///
/// The picks live in a file of their own so the menu never rewrites one a
/// person edits by hand, and a hand edit still beats a pick.
pub fn load_theme() -> theme::Theme {
    logged(load_theme_report())
}

/// The theme and what was wrong with it: one line per layer left out.
pub fn load_theme_report() -> (theme::Theme, Vec<String>) {
    let system = paths::system_config("pt35/theme.toml");
    let picked = paths::appearance_path();
    let user = paths::user_config("pt35/theme.toml");
    load_layers(&[system.as_deref(), Some(&picked), user.as_deref()])
}

fn logged<T>((value, skipped): (T, Vec<String>)) -> T {
    for problem in skipped {
        log::warn!("{problem}; that file is left out");
    }
    value
}

/// Record Appearance picks in one `section`, keeping the rest of the file.
///
/// `replace` drops what the section held first: a palette is every colour at
/// once, and an accent picked under the last palette must not outlive it.
pub fn set_appearance(
    section: &str,
    entries: &[(&str, toml::Value)],
    replace: bool,
) -> anyhow::Result<()> {
    let path = paths::appearance_path();
    let mut doc: toml::Table = match std::fs::read_to_string(&path) {
        // Refused rather than started afresh: that would throw away every
        // earlier pick without a word.
        Ok(text) => toml::from_str(&text).map_err(|e| {
            anyhow::anyhow!("{} does not parse ({e}); fix or delete it", path.display())
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => anyhow::bail!("reading {}: {e}", path.display()),
    };
    if replace {
        doc.remove(section);
    }
    let table = doc
        .entry(section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(table) = table {
        for (key, value) in entries {
            table.insert(key.to_string(), value.clone());
        }
    }
    // Checked before writing: a pick that breaks the theme would take every
    // colour back to the defaults with no word of why.
    toml::Value::Table(doc.clone()).try_into::<theme::Theme>()?;
    let text = format!(
        "# Written by Settings > Appearance. theme.toml next to it wins over this.\n{}",
        toml::to_string(&doc)?
    );
    write_atomic(&path, text.as_bytes())?;
    Ok(())
}

/// Write a file whole or not at all: to a temporary file beside it, then a
/// rename. A handheld loses power mid-write, and half a file parses as nothing.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let result = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()
    });
    match result.and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Merge the layers in order. One that cannot be read, does not parse, or
/// breaks the result is left out and named, and the rest still apply: a typo
/// in one file must not cost the other two.
fn load_layers<T: serde::de::DeserializeOwned + Default>(
    layers: &[Option<&std::path::Path>],
) -> (T, Vec<String>) {
    let mut doc = toml::Value::Table(toml::map::Map::new());
    let mut skipped = Vec::new();
    for path in layers.iter().copied().flatten() {
        if !path.exists() {
            continue;
        }
        let value = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str::<toml::Value>(&text).map_err(|e| e.to_string()));
        let value = match value {
            Ok(value) => value,
            Err(e) => {
                skipped.push(format!("{}: {}", tidy(path), one_line(&e)));
                continue;
            }
        };
        let mut next = doc.clone();
        merge_toml(&mut next, value);
        match next.clone().try_into::<T>() {
            Ok(_) => doc = next,
            Err(e) => skipped.push(format!("{}: {}", tidy(path), one_line(&e.to_string()))),
        }
    }
    (doc.try_into().unwrap_or_default(), skipped)
}

/// A path as a person would type it: `~/.config/pt35/theme.toml`.
fn tidy(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A TOML error is several lines with the source quoted. One line is what fits
/// in a banner and a log: where, then what.
fn one_line(error: &str) -> String {
    let lines: Vec<&str> = error
        .trim_start_matches("TOML parse error at ")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match (lines.first(), lines.last()) {
        (Some(first), Some(last)) if first != last => format!("{first}: {last}"),
        (Some(first), _) => first.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Default, Debug, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct Conf {
        a: u32,
        b: u32,
    }

    fn file(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_bad_layer_costs_only_itself() {
        let dir = std::env::temp_dir().join(format!("pt35-layers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let system = file(&dir, "system.toml", "a = 1\nb = 2\n");
        let picked = file(&dir, "picked.toml", "b = 3\n");
        let broken = file(&dir, "broken.toml", "a = [unclosed\n");
        let stale = file(&dir, "stale.toml", "a = 9\nremoved_key = true\n");
        let layers = [
            Some(system.as_path()),
            Some(&picked),
            Some(&broken),
            Some(&stale),
        ];
        let (conf, skipped): (Conf, _) = load_layers(&layers);
        assert_eq!(conf, Conf { a: 1, b: 3 });
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(skipped[0].contains("broken.toml"));
        assert!(!skipped[0].contains('\n'), "one line: {}", skipped[0]);
        assert!(skipped[1].contains("stale.toml"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_nothing_beside_it() {
        let dir = std::env::temp_dir().join(format!("pt35-atomic-{}", std::process::id()));
        let path = dir.join("pinned");
        write_atomic(&path, b"one\n").unwrap();
        write_atomic(&path, b"two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
