//! The root-owned allowlist: Store app id -> packages to install / remove.
//! Re-read on every request. Unsafe permissions refuse the whole file.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub install: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    #[serde(default = "yes")]
    pub update: bool,
}

pub type Allowlist = BTreeMap<String, Entry>;

fn yes() -> bool {
    true
}

/// `^[a-z0-9][a-z0-9-]{0,63}$`
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Debian policy name `^[a-z0-9][a-z0-9+.-]+$`, optionally followed by `:arm64`.
pub fn valid_package(name: &str) -> bool {
    let base = name.strip_suffix(":arm64").unwrap_or(name);
    let b = base.as_bytes();
    b.len() >= 2
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'+' | b'.' | b'-'))
}

pub fn parse(text: &str) -> Result<Allowlist, String> {
    let list: Allowlist = toml::from_str(text).map_err(|e| format!("allowlist: {}", e.message()))?;
    for (id, e) in &list {
        if !valid_id(id) {
            return Err(format!("allowlist: bad id {id:?}"));
        }
        if e.install.is_empty() {
            return Err(format!("allowlist: [{id}] install is empty"));
        }
        if let Some(p) = e.install.iter().chain(&e.remove).find(|p| !valid_package(p)) {
            return Err(format!("allowlist: [{id}] bad package name {p:?}"));
        }
    }
    Ok(list)
}

/// Every directory above the (canonical) file, and the file, must be root-owned and not group/other-writable.
fn check_permissions(canon: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    for p in canon.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        let md = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
        if md.uid() != 0 {
            return Err(format!("{} is not owned by root", p.display()));
        }
        if md.mode() & 0o022 != 0 {
            return Err(format!("{} is writable by group or others", p.display()));
        }
    }
    Ok(())
}

/// Resolves the path once, checks that resolved path, and reads that same path (no swap race).
pub fn load(path: &Path, test_mode: bool) -> Result<Allowlist, String> {
    let canon = std::fs::canonicalize(path).map_err(|e| format!("allowlist {}: {e}", path.display()))?;
    if !test_mode {
        check_permissions(&canon)?;
    }
    let text = std::fs::read_to_string(&canon).map_err(|e| format!("allowlist {}: {e}", canon.display()))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid() {
        let a = parse("[root-test]\ninstall=[\"sl\"]\n[kodi]\ninstall=[\"kodi\",\"libfoo:arm64\"]\nremove=[\"kodi\"]\nupdate=false\n").unwrap();
        assert_eq!(a["root-test"], Entry { install: vec!["sl".into()], remove: vec![], update: true });
        assert!(!a["kodi"].update);
        assert_eq!(a["kodi"].remove, vec!["kodi"]);
    }

    #[test]
    fn rejects_bad_ids() {
        for id in ["Bad", "-x", "a_b", "a b", "", &"a".repeat(65)] {
            let t = format!("[\"{id}\"]\ninstall=[\"sl\"]\n");
            assert!(parse(&t).is_err(), "{id}");
        }
        assert!(parse(&format!("[\"{}\"]\ninstall=[\"sl\"]\n", "a".repeat(64))).is_ok());
    }

    #[test]
    fn rejects_bad_packages() {
        for p in ["-y", "--force-yes", "Sl", "s", "a b", "a;b", "a/b", "x:amd64", "a$(x)", ""] {
            let t = format!("[a]\ninstall=[\"{p}\"]\n");
            assert!(parse(&t).is_err(), "{p}");
            let t = format!("[a]\ninstall=[\"sl\"]\nremove=[\"{p}\"]\n");
            assert!(parse(&t).is_err(), "remove {p}");
        }
    }

    #[test]
    fn rejects_structure_errors() {
        assert!(parse("[a]\ninstall=[]\n").is_err());
        assert!(parse("[a]\nremove=[\"sl\"]\n").is_err());
        assert!(parse("[a]\ninstall=[\"sl\"]\nextra=1\n").is_err());
        assert!(parse("top=1\n").is_err());
    }

    #[test]
    fn empty_file_is_empty_list() {
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn non_root_owned_file_refused() {
        // The test runs as a normal user, so a temp file is not root-owned.
        if unsafe { libc::geteuid() } != 0 {
            let dir = std::env::temp_dir().join(format!("lh-perm-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join("a.toml");
            std::fs::write(&f, "[a]\ninstall=[\"sl\"]\n").unwrap();
            assert!(load(&f, false).is_err());
            assert!(load(&f, true).is_ok());
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn package_name_boundaries() {
        for ok in ["ab", "a+", "g++", "libstdc++6", "a.b-c", "ab:arm64", "0ad", "gstreamer1.0-libav"] {
            assert!(valid_package(ok), "{ok}");
        }
        for bad in ["a", "+ab", ".ab", "-ab", "ab:armhf", ":arm64", "ab:arm64:arm64", "ab:", "aB", "ab\n", "ab\0", "a=1.0", "a*", "ab_c"] {
            assert!(!valid_package(bad), "{bad:?}");
        }
    }

    #[test]
    fn id_boundaries() {
        for ok in ["0a", "a-", "a--b", "kodi", "root-test"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "A", "-", "a.b", "a/b", "../x", "a\n", "é"] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn update_must_be_a_bool_and_defaults_on() {
        assert!(parse("[a]\ninstall=[\"sl\"]\nupdate=\"no\"\n").is_err());
        assert!(parse("[a]\ninstall=[\"sl\"]\n").unwrap()["a"].update);
    }

    #[test]
    fn duplicate_tables_are_rejected() {
        assert!(parse("[a]\ninstall=[\"sl\"]\n[a]\ninstall=[\"sl\"]\n").is_err());
    }

    #[test]
    fn load_reports_a_missing_file_and_follows_symlinks_in_test_mode() {
        let dir = std::env::temp_dir().join(format!("lh-load-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir.join("nope.toml"), true).unwrap_err().contains("allowlist"));
        std::fs::write(dir.join("real.toml"), "[a]\ninstall=[\"sl\"]\n").unwrap();
        std::os::unix::fs::symlink(dir.join("real.toml"), dir.join("link.toml")).unwrap();
        assert!(load(&dir.join("link.toml"), true).unwrap().contains_key("a"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shipped_allowlist_parses_and_keeps_its_safety_properties() {
        let a = parse(include_str!("../../session/helper-allowlist.toml")).unwrap();
        let ids: Vec<_> = a.keys().map(String::as_str).collect();
        assert_eq!(ids, ["airplay", "kodi", "root-test", "steamlink"]);
        // AirPlay's packages are shared with Lumen's tile and other things: never removable.
        assert!(a["airplay"].remove.is_empty());
        // Kodi: apt must not be free to pull RetroArch in or take extra packages out, so every
        // installed package is removable and so is the visualization apt takes along.
        for p in &a["kodi"].install {
            assert!(a["kodi"].remove.contains(p), "{p} missing from kodi remove");
        }
        assert!(a["kodi"].remove.contains(&"kodi-visualization-spectrum".to_string()));
        // A Recommends of kodi; --no-install-recommends would otherwise skip it.
        assert!(a["kodi"].install.contains(&"kodi-repository-kodi".to_string()));
        // No set may name an apt option or a purge-like escape hatch as a package.
        for (id, e) in &a {
            assert!(e.install.iter().chain(&e.remove).all(|p| !p.starts_with('-')), "{id}");
        }
    }
}
