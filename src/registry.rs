// App registry: one TOML file per app in ~/.config/lumen-tv/apps/*.toml. The
// defaults in config/apps/ are built in and written out on first start only.
// A broken file is skipped and reported as data (`Loaded.errors`, for the M3.5
// Settings screen), never fatal.
//
// Fields (BRIEF s5 plus Lumen additions): id, name, type, launch, url,
// fallback_url, background, exclusive, icon, app_id, glyph, colors, order, and
// `process`: the path (or comm) of the app's real executable, for launchers
// that exit at once (Steam Link). Resolving `icon` belongs to icons.rs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The apps built into Lumen.
pub const SHIPPED: [(&str, &str); 4] = [
    ("jellyfin.toml", include_str!("../config/apps/jellyfin.toml")),
    ("plex.toml", include_str!("../config/apps/plex.toml")),
    ("steamlink.toml", include_str!("../config/apps/steamlink.toml")),
    ("kodi.toml", include_str!("../config/apps/kodi.toml")),
];

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Command,
    Url,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    KeepAlive,
    #[default]
    CloseOnExit,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: Kind,
    /// Run as `sh -c 'exec <launch>'`. Required for `command`.
    pub launch: Option<String>,
    /// Opened with the kiosk browser. Required for `url`.
    pub url: Option<String>,
    /// Used when `launch`'s program isn't on PATH.
    pub fallback_url: Option<String>,
    #[serde(default)]
    pub background: Background,
    #[serde(default)]
    pub exclusive: bool,
    pub icon: Option<String>,
    /// Wayland app_id / X11 class of the app's windows.
    pub app_id: Option<String>,
    /// The real app's executable (`~` expanded), matched by /proc/<pid>/exe,
    /// or by comm when it has no `/`. For launchers that exit immediately.
    pub process: Option<String>,
    /// 1-2 characters, shown when there's no icon.
    pub glyph: Option<String>,
    /// 2-3 `#rrggbb` stops for the tile gradient.
    pub colors: Option<Vec<String>>,
    pub order: Option<i32>,
}

impl Entry {
    fn validate(&self) -> Result<(), String> {
        if self.id.is_empty() || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!("id {:?} may only use letters, digits, - and _", self.id));
        }
        if self.name.trim().is_empty() {
            return Err("name is empty".into());
        }
        let blank = |o: &Option<String>| o.as_deref().is_none_or(|s| s.trim().is_empty());
        match self.kind {
            Kind::Command if blank(&self.launch) => return Err("type \"command\" needs a launch line".into()),
            Kind::Url if blank(&self.url) => return Err("type \"url\" needs a url".into()),
            _ => {}
        }
        if self.process.as_deref().is_some_and(|s| s.trim().is_empty()) {
            return Err("process is empty".into());
        }
        if let Some(g) = &self.glyph {
            if !(1..=2).contains(&g.chars().count()) {
                return Err(format!("glyph {g:?} should be 1-2 characters"));
            }
        }
        if let Some(colors) = &self.colors {
            if !(2..=3).contains(&colors.len()) {
                return Err("colors needs 2 or 3 entries".into());
            }
            if let Some(c) = colors.iter().find(|c| !is_hex_color(c)) {
                return Err(format!("color {c:?} should look like \"#rrggbb\""));
            }
        }
        Ok(())
    }
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Parses and validates one registry file.
pub fn parse(file: &str, text: &str) -> Result<Entry, String> {
    let entry: Entry = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
    entry.validate().map_err(|e| format!("{file}: {e}"))?;
    Ok(entry)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub message: String,
}

#[derive(Default, Debug)]
pub struct Loaded {
    /// Sorted by `order`, then name.
    pub entries: Vec<Entry>,
    /// Files that were skipped, also logged.
    pub errors: Vec<LoadError>,
}

/// `$XDG_CONFIG_HOME` or `~/.config`, plus `lumen-tv`.
pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("lumen-tv")
}

pub fn apps_dir() -> PathBuf {
    config_dir().join("apps")
}

/// Writes the built-in files, but only if `dir` doesn't exist yet.
/// Returns whether it wrote them. Never overwrites anything.
pub fn seed_defaults(dir: &Path) -> std::io::Result<bool> {
    if dir.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    for (name, text) in SHIPPED {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(true)
}

/// Reads every `*.toml` in `dir` (in file-name order, so the first of two
/// duplicate ids wins). A missing directory is an empty registry.
pub fn load(dir: &Path) -> Loaded {
    let mut files: Vec<(String, std::io::Result<String>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".toml") {
                files.push((name, std::fs::read_to_string(e.path())));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let loaded = load_texts(files.into_iter().map(|(n, r)| (n, r.map_err(|e| e.to_string()))));
    for e in &loaded.errors {
        eprintln!("[registry] skipped {}: {}", e.file, e.message);
    }
    loaded
}

/// The pure part of `load`: (file name, text or read error) in, entries and errors out.
pub fn load_texts(files: impl IntoIterator<Item = (String, Result<String, String>)>) -> Loaded {
    let mut out = Loaded::default();
    let mut seen: HashSet<String> = HashSet::new();
    for (file, text) in files {
        let res = text.and_then(|t| parse(&file, &t)).and_then(|entry| {
            if seen.insert(entry.id.clone()) {
                Ok(entry)
            } else {
                Err(format!("{file}: duplicate id {:?}", entry.id))
            }
        });
        match res {
            Ok(entry) => out.entries.push(entry),
            Err(message) => out.errors.push(LoadError { file, message }),
        }
    }
    out.entries.sort_by_cached_key(|e| (e.order.unwrap_or(i32::MAX), e.name.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(list: &[(&str, &str)]) -> Loaded {
        load_texts(list.iter().map(|(n, t)| (n.to_string(), Ok(t.to_string()))))
    }

    #[test]
    fn every_shipped_file_parses() {
        for (name, text) in SHIPPED {
            let e = parse(name, text).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(format!("{}.toml", e.id), name, "id should be the file stem");
        }
    }

    #[test]
    fn shipped_set_matches_the_design() {
        let l = load_texts(SHIPPED.iter().map(|(n, t)| (n.to_string(), Ok(t.to_string()))));
        assert!(l.errors.is_empty());
        let ids: Vec<_> = l.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["jellyfin", "plex", "steamlink", "kodi"]);
        let by = |id: &str| l.entries.iter().find(|e| e.id == id).unwrap();
        assert_eq!(by("plex").kind, Kind::Url);
        assert_eq!(by("plex").background, Background::KeepAlive);
        assert!(by("steamlink").exclusive && by("kodi").exclusive);
        assert!(by("steamlink").process.is_some());
        assert!(by("plex").icon.is_none());
    }

    #[test]
    fn defaults_apply() {
        let e = parse("a.toml", "id = \"a\"\nname = \"A\"\nlaunch = \"a\"\n").unwrap();
        assert_eq!(e.kind, Kind::Command);
        assert_eq!(e.background, Background::CloseOnExit);
        assert!(!e.exclusive);
        assert!(e.app_id.is_none() && e.process.is_none() && e.order.is_none());
    }

    #[test]
    fn unknown_key_rejected() {
        let err = parse("a.toml", "id = \"a\"\nname = \"A\"\nlaunch = \"a\"\nbogus = 1\n").unwrap_err();
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn missing_launch_or_url_rejected() {
        assert!(parse("a.toml", "id = \"a\"\nname = \"A\"\n").unwrap_err().contains("launch"));
        assert!(parse("a.toml", "id = \"a\"\nname = \"A\"\ntype = \"url\"\nlaunch = \"x\"\n").unwrap_err().contains("url"));
    }

    #[test]
    fn bad_values_rejected() {
        let base = "id = \"a\"\nname = \"A\"\nlaunch = \"a\"\n";
        assert!(parse("a.toml", &format!("{base}glyph = \"abc\"\n")).is_err());
        assert!(parse("a.toml", &format!("{base}colors = [\"#fff\", \"#000000\"]\n")).is_err());
        assert!(parse("a.toml", &format!("{base}colors = [\"#ffffff\"]\n")).is_err());
        assert!(parse("a.toml", "id = \"a b\"\nname = \"A\"\nlaunch = \"a\"\n").is_err());
        assert!(parse("a.toml", &format!("{base}background = \"forever\"\n")).is_err());
    }

    #[test]
    fn ordering_is_order_then_name() {
        let l = texts(&[
            ("z.toml", "id = \"z\"\nname = \"Zed\"\nlaunch = \"z\"\norder = 1\n"),
            ("b.toml", "id = \"b\"\nname = \"bravo\"\nlaunch = \"b\"\n"),
            ("a.toml", "id = \"a\"\nname = \"Alpha\"\nlaunch = \"a\"\n"),
            ("c.toml", "id = \"c\"\nname = \"Charlie\"\nlaunch = \"c\"\norder = 1\n"),
        ]);
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Charlie", "Zed", "Alpha", "bravo"]);
    }

    #[test]
    fn errors_are_data_and_dont_stop_the_rest() {
        let l = load_texts(vec![
            ("bad.toml".to_string(), Ok("not toml [".to_string())),
            ("unreadable.toml".to_string(), Err("permission denied".to_string())),
            ("ok.toml".to_string(), Ok("id = \"ok\"\nname = \"Ok\"\nlaunch = \"ok\"\n".to_string())),
        ]);
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.errors.len(), 2);
        assert_eq!(l.errors[0].file, "bad.toml");
        assert_eq!(l.errors[1].message, "permission denied");
    }

    #[test]
    fn duplicate_ids_rejected() {
        let l = texts(&[
            ("a.toml", "id = \"x\"\nname = \"First\"\nlaunch = \"a\"\n"),
            ("b.toml", "id = \"x\"\nname = \"Second\"\nlaunch = \"b\"\n"),
        ]);
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.entries[0].name, "First");
        assert!(l.errors[0].message.contains("duplicate"));
    }

    #[test]
    fn blank_required_fields_rejected() {
        let ok = "id = \"a\"\nname = \"A\"\n";
        assert!(parse("a.toml", "id = \"\"\nname = \"A\"\nlaunch = \"a\"\n").is_err(), "empty id");
        assert!(parse("a.toml", "id = \"a\"\nname = \"  \"\nlaunch = \"a\"\n").unwrap_err().contains("name"));
        assert!(parse("a.toml", &format!("{ok}launch = \"  \"\n")).unwrap_err().contains("launch"));
        assert!(parse("a.toml", &format!("{ok}type = \"url\"\nurl = \"\"\n")).unwrap_err().contains("url"));
        assert!(parse("a.toml", &format!("{ok}launch = \"a\"\nprocess = \" \"\n")).unwrap_err().contains("process"));
        assert!(parse("a.toml", &format!("{ok}launch = \"a\"\nglyph = \"\"\n")).is_err(), "empty glyph");
    }

    #[test]
    fn value_boundaries_accepted() {
        let base = "id = \"a_b-1\"\nname = \"A\"\nlaunch = \"a\"\n";
        assert!(parse("a.toml", &format!("{base}glyph = \"AB\"\n")).is_ok());
        assert!(parse("a.toml", &format!("{base}colors = [\"#AABBCC\", \"#001122\", \"#ffffff\"]\n")).is_ok());
        assert!(parse("a.toml", &format!("{base}colors = [\"#000000\", \"#111111\", \"#222222\", \"#333333\"]\n")).is_err());
        assert!(parse("a.toml", &format!("{base}colors = [\"#00000g\", \"#111111\"]\n")).is_err());
        assert!(parse("a.toml", &format!("{base}type = \"window\"\n")).is_err(), "unknown type");
    }

    #[test]
    fn url_entry_may_omit_launch() {
        let e = parse("a.toml", "id = \"a\"\nname = \"A\"\ntype = \"url\"\nurl = \"http://x\"\n").unwrap();
        assert_eq!(e.kind, Kind::Url);
        assert!(e.launch.is_none());
    }

    fn temp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lumen-registry-{tag}-{}", std::process::id()))
    }

    #[test]
    fn seed_writes_once_and_never_overwrites() {
        let dir = temp("seed");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seed_defaults(&dir).unwrap());
        assert_eq!(load(&dir).entries.len(), 4);
        std::fs::write(dir.join("kodi.toml"), "mine").unwrap();
        assert!(!seed_defaults(&dir).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("kodi.toml")).unwrap(), "mine");
        let l = load(&dir);
        assert_eq!(l.entries.len(), 3);
        assert_eq!(l.errors.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_dir_is_empty() {
        let l = load(&temp("nonexistent"));
        assert!(l.entries.is_empty() && l.errors.is_empty());
    }
}
