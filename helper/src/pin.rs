//! PIN hashing (argon2id) and the persisted failure counter / lockout.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const FREE_TRIES: u32 = 5;
pub const BASE_LOCK_S: u64 = 60;
pub const MAX_LOCK_S: u64 = 3600;

fn argon() -> Argon2<'static> {
    // m = 19 MiB, t = 2, p = 1
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(19 * 1024, 2, 1, None).expect("argon2 params"))
}

pub fn hash_pin(pin: &str) -> Result<String, String> {
    let mut raw = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut raw))
        .map_err(|e| format!("random: {e}"))?;
    let salt = SaltString::encode_b64(&raw).map_err(|e| e.to_string())?;
    argon().hash_password(pin.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

/// Constant-time verify (done by the crate).
pub fn verify_pin(pin: &str, stored: &str) -> bool {
    match PasswordHash::new(stored.trim()) {
        Ok(h) => argon().verify_password(pin.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempts {
    pub failures: u32,
    pub locked_until: u64,
}

pub fn lock_duration(failures: u32) -> u64 {
    if failures < FREE_TRIES {
        return 0;
    }
    BASE_LOCK_S.saturating_mul(1u64 << (failures - FREE_TRIES).min(20)).min(MAX_LOCK_S)
}

impl Attempts {
    pub fn locked_for(&self, now: u64) -> u64 {
        self.locked_until.saturating_sub(now).min(MAX_LOCK_S)
    }
    pub fn record_failure(&mut self, now: u64) {
        self.failures = self.failures.saturating_add(1);
        let d = lock_duration(self.failures);
        if d > 0 {
            self.locked_until = now + d;
        }
    }
    pub fn tries_left(&self) -> Option<u32> {
        (self.failures < FREE_TRIES).then(|| FREE_TRIES - self.failures)
    }
    pub fn reset(&mut self) {
        *self = Attempts::default();
    }

    /// Missing file = no failures. A file that exists but can't be parsed is an error (fail closed).
    /// A lock further out than the maximum (the Pi has no RTC, so the clock can jump back) is capped.
    pub fn load(path: &Path, now: u64) -> Result<Attempts, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => {
                let mut a: Attempts = serde_json::from_str(&t).map_err(|_| "attempt state unreadable".to_string())?;
                a.locked_until = a.locked_until.min(now.saturating_add(MAX_LOCK_S));
                Ok(a)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Attempts::default()),
            Err(e) => Err(format!("attempt state: {e}")),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        write_private(path, serde_json::to_string(self).map_err(|e| e.to_string())?.as_bytes())
    }
}

/// Atomic write (tmp + rename), mode 0600.
pub fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let res = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    res.map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash_pin("1234").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_pin("1234", &h));
        assert!(!verify_pin("1235", &h));
        assert!(!verify_pin("1234", "garbage"));
        assert_ne!(h, hash_pin("1234").unwrap(), "salted");
    }

    #[test]
    fn lock_math() {
        assert_eq!(lock_duration(4), 0);
        assert_eq!(lock_duration(5), 60);
        assert_eq!(lock_duration(6), 120);
        assert_eq!(lock_duration(7), 240);
        assert_eq!(lock_duration(11), 3600);
        assert_eq!(lock_duration(500), 3600);
    }

    #[test]
    fn failures_then_lock_then_reset() {
        let mut a = Attempts::default();
        for i in 1..5 {
            a.record_failure(1000);
            assert_eq!(a.locked_for(1000), 0);
            assert_eq!(a.tries_left(), Some(5 - i));
        }
        a.record_failure(1000);
        assert_eq!(a.locked_for(1000), 60);
        assert_eq!(a.locked_for(1059), 1);
        assert_eq!(a.locked_for(1060), 0);
        assert_eq!(a.tries_left(), None);
        a.record_failure(1060);
        assert_eq!(a.locked_for(1060), 120);
        a.reset();
        assert_eq!(a, Attempts::default());
    }

    #[test]
    fn persists_across_reload() {
        let dir = std::env::temp_dir().join(format!("lh-att-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("attempts.json");
        assert_eq!(Attempts::load(&f, 0).unwrap(), Attempts::default());
        let mut a = Attempts::default();
        for _ in 0..5 {
            a.record_failure(5000);
        }
        a.save(&f).unwrap();
        let b = Attempts::load(&f, 5000).unwrap();
        assert_eq!(a, b);
        assert_eq!(b.locked_for(5000), 60);
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&f, "junk").unwrap();
        assert!(Attempts::load(&f, 0).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn far_future_lock_is_capped_on_load() {
        let dir = std::env::temp_dir().join(format!("lh-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("attempts.json");
        Attempts { failures: 5, locked_until: u64::MAX }.save(&f).unwrap();
        let a = Attempts::load(&f, 1000).unwrap();
        assert_eq!(a.locked_until, 1000 + MAX_LOCK_S);
        assert_eq!(a.locked_for(1000), MAX_LOCK_S);
        assert_eq!(a.locked_for(1000 + MAX_LOCK_S), 0, "the cap lets it expire");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_is_strict_about_the_pin_and_tolerant_of_a_trailing_newline_in_the_file() {
        let h = hash_pin("2468").unwrap();
        assert!(verify_pin("2468", &format!("{h}\n")), "set-pin may leave a newline");
        assert!(!verify_pin("", &h));
        assert!(!verify_pin("2468 ", &h));
        assert!(!verify_pin("24680", &h));
        assert!(!verify_pin("2468", ""));
        assert!(!verify_pin("2468", "\n"));
        assert!(!verify_pin("2468", &h[..h.len() - 4]), "truncated hash");
    }

    #[test]
    fn lock_duration_never_overflows_and_stays_capped() {
        assert_eq!(lock_duration(0), 0);
        assert_eq!(lock_duration(u32::MAX), MAX_LOCK_S);
        assert_eq!(lock_duration(FREE_TRIES + 21), MAX_LOCK_S);
        let mut last = 0;
        for f in 0..200 {
            let d = lock_duration(f);
            assert!(d >= last && d <= MAX_LOCK_S, "{f}: {d}");
            last = d;
        }
    }

    #[test]
    fn failure_counter_saturates_and_locked_for_is_capped() {
        let mut a = Attempts { failures: u32::MAX, locked_until: 0 };
        a.record_failure(10);
        assert_eq!(a.failures, u32::MAX);
        assert_eq!(a.locked_for(10), MAX_LOCK_S);
        let far = Attempts { failures: 5, locked_until: u64::MAX };
        assert_eq!(far.locked_for(0), MAX_LOCK_S);
    }

    #[test]
    fn an_expired_lock_still_counts_toward_the_next_lock() {
        let mut a = Attempts::default();
        for _ in 0..5 {
            a.record_failure(100);
        }
        assert_eq!(a.locked_for(160), 0, "lock over");
        assert_eq!(a.tries_left(), None, "but no free tries come back");
        a.record_failure(160);
        assert_eq!(a.locked_for(160), 120);
    }

    #[test]
    fn corrupt_or_partial_attempt_files_fail_closed() {
        let dir = std::env::temp_dir().join(format!("lh-bad-att-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("attempts.json");
        for junk in ["", "{}", "{\"failures\":1}", "{\"failures\":-1,\"locked_until\":0}", "[]", "null"] {
            std::fs::write(&f, junk).unwrap();
            assert!(Attempts::load(&f, 0).is_err(), "{junk:?}");
        }
        // A directory where the file should be is an error, not "no failures".
        std::fs::remove_file(&f).unwrap();
        std::fs::create_dir(&f).unwrap();
        assert!(Attempts::load(&f, 0).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_private_replaces_atomically_with_mode_0600_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lh-wp-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("pin");
        write_private(&f, b"first").unwrap();
        write_private(&f, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "second");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, ["pin"]);
        assert!(write_private(&dir.join("missing-dir/pin"), b"x").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
