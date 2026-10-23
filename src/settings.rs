//! Lumen's saved settings: `config_dir()/settings.toml`.
//!
//! Two optional keys, `mode` (`WIDTHxHEIGHT@RATE`) and `store_url`. A load
//! never writes; a file with unknown keys or a bad value gives defaults plus an
//! error string for the Settings screen, and a save refuses to overwrite it.
//! `start-lumen.sh` greps the mode line, so `mode = "1920x1080@60"` is written
//! exactly in that form.

use std::path::{Path, PathBuf};

use crate::registry;

/// Where a resolved setting came from (shown in Settings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Settings,
    Env,
    Default,
}

impl Origin {
    /// "settings.toml", "environment" or "default".
    pub fn label(self) -> &'static str {
        match self {
            Origin::Settings => "settings.toml",
            Origin::Env => "environment",
            Origin::Default => "default",
        }
    }
}

/// A mode the Display panel offers: label, then the `WIDTHxHEIGHT@RATE` value.
pub const MODES: [(&str, &str); 2] = [("1080p60", "1920x1080@60"), ("4K60", "3840x2160@60")];

/// The default mode (first of [`MODES`]).
pub const DEFAULT_MODE: &str = "1920x1080@60";

/// Label for a mode value ("1080p60"), or the value itself if it isn't one of [`MODES`].
pub fn mode_label(mode: &str) -> String {
    MODES.iter().find(|(_, v)| *v == mode).map_or_else(|| mode.to_string(), |(l, _)| l.to_string())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub mode: Option<String>,
    pub store_url: Option<String>,
}

/// `settings.toml` in the config dir.
pub fn path() -> PathBuf {
    registry::config_dir().join("settings.toml")
}

/// True for `WIDTHxHEIGHT@RATE` (e.g. `1920x1080@60`, `3840x2160@59.94`).
pub fn valid_mode(m: &str) -> bool {
    let Some((res, rate)) = m.split_once('@') else { return false };
    let Some((w, h)) = res.split_once('x') else { return false };
    let num = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u32>().is_ok_and(|n| n > 0);
    num(w) && num(h) && rate.parse::<f64>().is_ok_and(|r| r.is_finite() && r > 0.0)
}

/// Parses file text. Unknown keys, wrong types and a malformed mode are errors.
pub fn parse(text: &str) -> Result<Settings, String> {
    let s: Settings = toml::from_str(text).map_err(|e| format!("settings.toml: {}", e.message()))?;
    if let Some(m) = &s.mode {
        if !valid_mode(m) {
            return Err(format!("settings.toml: bad mode {m:?}, expected like 1920x1080@60"));
        }
    }
    Ok(s)
}

/// Loads `path`. Missing file = defaults, no error. Any problem = defaults and
/// the error text; the file is never touched.
pub fn load_from(path: &Path) -> (Settings, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (Settings::default(), Some(format!("can't read {}: {e}", path.display()))),
    }
}

/// [`load_from`] the real settings file.
pub fn load() -> (Settings, Option<String>) {
    load_from(&path())
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The file text for `s`: one line per set key, `mode` first.
pub fn render(s: &Settings) -> String {
    let mut out = String::new();
    if let Some(m) = &s.mode {
        out.push_str(&format!("mode = {}\n", quote(m)));
    }
    if let Some(u) = &s.store_url {
        out.push_str(&format!("store_url = {}\n", quote(u)));
    }
    out
}

/// Atomic write (temp file + rename).
pub fn write_to(path: &Path, s: &Settings) -> Result<(), String> {
    let dir = path.parent().ok_or("settings path has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render(s)).map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("can't save {}: {e}", path.display())
    })
}

fn update(path: &Path, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let (mut s, err) = load_from(path);
    if let Some(e) = err {
        return Err(format!("not saving over a file I can't read ({e})"));
    }
    f(&mut s);
    write_to(path, &s)
}

/// Saves `mode` (must be `WIDTHxHEIGHT@RATE`) in `path`, keeping `store_url`.
pub fn save_mode_to(path: &Path, mode: &str) -> Result<(), String> {
    if !valid_mode(mode) {
        return Err(format!("bad mode {mode:?}"));
    }
    update(path, |s| s.mode = Some(mode.to_string()))
}

/// [`save_mode_to`] the real settings file.
pub fn save_mode(mode: &str) -> Result<(), String> {
    save_mode_to(&path(), mode)
}

/// settings `mode`, else env `LUMEN_MODE`, else [`DEFAULT_MODE`]. Pure.
pub fn resolve_mode(s: &Settings, env: Option<&str>) -> (String, Origin) {
    if let Some(m) = &s.mode {
        return (m.clone(), Origin::Settings);
    }
    match env.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => (m.to_string(), Origin::Env),
        None => (DEFAULT_MODE.to_string(), Origin::Default),
    }
}

/// The mode to run at, from the real settings file and environment.
pub fn mode() -> (String, Origin) {
    let (s, err) = load();
    if let Some(e) = err {
        eprintln!("[settings] {e}");
    }
    resolve_mode(&s, std::env::var("LUMEN_MODE").ok().as_deref())
}

/// settings `store_url`, else env `LUMEN_STORE_URL`, else `default`. Pure.
pub fn resolve_store_url(s: &Settings, env: Option<&str>, default: &str) -> (String, Origin) {
    if let Some(u) = s.store_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        return (u.to_string(), Origin::Settings);
    }
    match env.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => (u.to_string(), Origin::Env),
        None => (default.to_string(), Origin::Default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.toml")
    }

    #[test]
    fn parse_both_keys_and_empty() {
        assert_eq!(parse("").unwrap(), Settings::default());
        let s = parse("mode = \"3840x2160@60\"\nstore_url = \"https://x/y.json\"\n").unwrap();
        assert_eq!(s.mode.as_deref(), Some("3840x2160@60"));
        assert_eq!(s.store_url.as_deref(), Some("https://x/y.json"));
    }

    #[test]
    fn unknown_key_and_bad_values_are_errors() {
        assert!(parse("volume = 3").unwrap_err().contains("settings.toml"));
        assert!(parse("mode = \"4k\"").unwrap_err().contains("bad mode"));
        assert!(parse("mode = 5").is_err());
        assert!(parse("mode = [").is_err());
    }

    #[test]
    fn valid_mode_shapes() {
        for ok in ["1920x1080@60", "3840x2160@59.94"] {
            assert!(valid_mode(ok), "{ok}");
        }
        for bad in ["", "1920x1080", "1920@60", "x1080@60", "0x0@60", "1920x1080@", "1920x1080@fast"] {
            assert!(!valid_mode(bad), "{bad}");
        }
    }

    #[test]
    fn save_round_trip_keeps_other_key_and_exact_line() {
        let p = tmp("roundtrip");
        update(&p, |s| s.store_url = Some("https://a/b.json".into())).unwrap();
        save_mode_to(&p, "3840x2160@60").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.lines().any(|l| l == "mode = \"3840x2160@60\""), "{text}");
        let (s, err) = load_from(&p);
        assert_eq!(err, None);
        assert_eq!(s.mode.as_deref(), Some("3840x2160@60"));
        assert_eq!(s.store_url.as_deref(), Some("https://a/b.json"));
        save_mode_to(&p, "1920x1080@60").unwrap();
        assert_eq!(load_from(&p).0.store_url.as_deref(), Some("https://a/b.json"));
        assert!(!p.with_extension("toml.tmp").exists());
    }

    #[test]
    fn save_rejects_bad_mode_and_never_overwrites_a_broken_file() {
        let p = tmp("broken");
        assert!(save_mode_to(&p, "huge").is_err());
        assert!(!p.exists());
        std::fs::write(&p, "mystery = 1\n").unwrap();
        let (s, err) = load_from(&p);
        assert_eq!(s, Settings::default());
        assert!(err.is_some());
        assert!(save_mode_to(&p, "1920x1080@60").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mystery = 1\n");
    }

    #[test]
    fn missing_file_is_defaults_without_error() {
        let p = tmp("missing");
        assert_eq!(load_from(&p), (Settings::default(), None));
        assert!(!p.exists());
    }

    #[test]
    fn mode_precedence() {
        let set = Settings { mode: Some("3840x2160@60".into()), store_url: None };
        let none = Settings::default();
        assert_eq!(resolve_mode(&set, Some("1920x1080@30")), ("3840x2160@60".into(), Origin::Settings));
        assert_eq!(resolve_mode(&none, Some("1920x1080@30")), ("1920x1080@30".into(), Origin::Env));
        assert_eq!(resolve_mode(&none, Some(" ")), (DEFAULT_MODE.into(), Origin::Default));
        assert_eq!(resolve_mode(&none, None), (DEFAULT_MODE.into(), Origin::Default));
    }

    #[test]
    fn store_url_precedence() {
        let set = Settings { mode: None, store_url: Some("https://s".into()) };
        let none = Settings::default();
        assert_eq!(resolve_store_url(&set, Some("https://e"), "d"), ("https://s".into(), Origin::Settings));
        assert_eq!(resolve_store_url(&none, Some("https://e"), "d"), ("https://e".into(), Origin::Env));
        assert_eq!(resolve_store_url(&none, Some(""), "d"), ("d".into(), Origin::Default));
    }

    #[test]
    fn labels() {
        assert_eq!(mode_label("3840x2160@60"), "4K60");
        assert_eq!(mode_label("800x600@60"), "800x600@60");
        assert_eq!(MODES[0].1, DEFAULT_MODE);
    }


    #[test]
    fn save_creates_a_missing_config_dir() {
        let base = tmp("nodir");
        let p = base.parent().unwrap().join("not").join("yet").join("settings.toml");
        assert!(!p.parent().unwrap().exists());
        save_mode_to(&p, "3840x2160@60").unwrap();
        assert_eq!(load_from(&p).0.mode.as_deref(), Some("3840x2160@60"));
        let _ = std::fs::remove_dir_all(base.parent().unwrap().join("not"));
    }

    #[test]
    fn broken_file_also_blocks_store_url_save() {
        let p = tmp("broken-url");
        std::fs::write(&p, "mode = \"huge\"\n").unwrap();
        assert!(update(&p, |s| s.store_url = Some("https://x".into())).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "mode = \"huge\"\n");
    }

    #[test]
    fn render_escapes_quotes_in_values() {
        let s = Settings { mode: None, store_url: Some("https://x/\"q\".json".into()) };
        assert_eq!(parse(&render(&s)).unwrap(), s);
    }

    #[test]
    fn blank_settings_store_url_falls_through_to_env_then_default() {
        let blank = Settings { mode: None, store_url: Some("   ".into()) };
        assert_eq!(resolve_store_url(&blank, Some(" https://e "), "d"), ("https://e".into(), Origin::Env));
        assert_eq!(resolve_store_url(&blank, None, "d"), ("d".into(), Origin::Default));
    }

    #[test]
    fn mode_rejects_zero_and_signs() {
        for bad in ["1920x1080@0", "1920x1080@-60", "-1x1080@60", "1920x1080@NaN", "1920x1080@inf", "1920x1080x2@60"] {
            assert!(!valid_mode(bad), "{bad}");
        }
    }
}
