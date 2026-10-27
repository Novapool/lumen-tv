//! lumen-helper: Lumen's small root daemon. Installs/removes allowlisted apt packages
//! for a Store app after a PIN check. Protocol and rules: AGENT_JOBS.md / README.

mod allowlist;
mod apt;
mod pin;
mod proto;
mod server;

use server::{Config, Server};
use std::io::{BufRead, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "usage: lumen-helper [serve|set-pin|check] [--socket P] [--state-dir D] [--allowlist F] [--apt BIN] [--idle-exit SECS] [--insecure-test-mode]";

fn parse_args(args: Vec<String>) -> Result<(String, Config), String> {
    let mut cmd = None;
    let mut cfg = Config {
        socket: None,
        state_dir: "/var/lib/lumen-helper".into(),
        allowlist: "/etc/lumen-tv/helper-allowlist.toml".into(),
        apt: "/usr/bin/apt-get".into(),
        idle: Duration::from_secs(60),
        test_mode: false,
    };
    let mut overridden = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        overridden |= matches!(a.as_str(), "--socket" | "--state-dir" | "--allowlist" | "--apt");
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "serve" | "set-pin" | "check" if cmd.is_none() => cmd = Some(a),
            "--socket" => cfg.socket = Some(val("--socket")?.into()),
            "--state-dir" => cfg.state_dir = val("--state-dir")?.into(),
            "--allowlist" => cfg.allowlist = val("--allowlist")?.into(),
            "--apt" => cfg.apt = val("--apt")?.into(),
            "--idle-exit" => cfg.idle = Duration::from_secs(val("--idle-exit")?.parse().map_err(|_| "--idle-exit needs seconds")?),
            "--insecure-test-mode" => cfg.test_mode = true,
            _ => return Err(format!("unknown argument {a:?}")),
        }
    }
    if overridden && !cfg.test_mode {
        return Err("--socket, --state-dir, --allowlist and --apt require --insecure-test-mode".into());
    }
    Ok((cmd.unwrap_or_else(|| "serve".into()), cfg))
}

fn require_root(cfg: &Config) -> Result<(), String> {
    if !cfg.test_mode && unsafe { libc::geteuid() } != 0 {
        return Err("must run as root".into());
    }
    Ok(())
}

fn ensure_state_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(())
}

fn listener(cfg: &Config) -> Result<UnixListener, String> {
    use std::os::fd::FromRawFd;
    let pid_ok = std::env::var("LISTEN_PID").ok().and_then(|p| p.parse::<u32>().ok()) == Some(std::process::id());
    if pid_ok && std::env::var("LISTEN_FDS").as_deref() == Ok("1") {
        unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) }; // never inherited by apt
        return Ok(unsafe { UnixListener::from_raw_fd(3) });
    }
    let path = cfg.socket.clone().unwrap_or_else(|| PathBuf::from("/run/lumen-helper.sock"));
    let _ = std::fs::remove_file(&path);
    let l = UnixListener::bind(&path).map_err(|e| format!("bind {}: {e}", path.display()))?;
    let mode = if cfg.test_mode { 0o600 } else { 0o660 };
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(mode)).map_err(|e| e.to_string())?;
    Ok(l)
}

fn read_line_noecho(prompt: &str, test_mode: bool) -> Result<String, String> {
    let mut line = String::new();
    if test_mode {
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        return Ok(line.trim_end().to_string());
    }
    use std::os::fd::AsRawFd;
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").map_err(|e| format!("no terminal: {e}"))?;
    let fd = tty.as_raw_fd();
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut old) } != 0 {
        return Err("cannot read terminal settings".into());
    }
    let mut quiet = old;
    quiet.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) };
    let _ = write!(tty, "{prompt}");
    let _ = tty.flush();
    let res = std::io::BufReader::new(&tty).read_line(&mut line);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &old) };
    let _ = writeln!(tty);
    res.map_err(|e| e.to_string())?;
    Ok(line.trim_end().to_string())
}

fn set_pin(cfg: &Config) -> Result<(), String> {
    require_root(cfg)?;
    let first = read_line_noecho("New PIN (4-8 digits): ", cfg.test_mode)?;
    if !proto::valid_pin(&first) {
        return Err("the PIN must be 4 to 8 digits".into());
    }
    if read_line_noecho("Repeat PIN: ", cfg.test_mode)? != first {
        return Err("the PINs do not match".into());
    }
    ensure_state_dir(&cfg.state_dir)?;
    pin::write_private(&cfg.pin_file(), pin::hash_pin(&first)?.as_bytes())?;
    pin::Attempts::default().save(&cfg.attempts_file())?;
    eprintln!("PIN set.");
    Ok(())
}

fn check(cfg: &Config) -> Result<(), String> {
    let al = allowlist::load(&cfg.allowlist, cfg.test_mode)?;
    for (id, e) in &al {
        println!("[{id}] install={:?} remove={:?} update={}", e.install, e.remove, e.update);
    }
    println!("{} app(s); pin {}", al.len(), if cfg.pin_file().exists() { "set" } else { "NOT set" });
    Ok(())
}

fn serve(cfg: Config) -> Result<(), String> {
    require_root(&cfg)?;
    ensure_state_dir(&cfg.state_dir)?;
    let l = listener(&cfg)?;
    Arc::new(Server::new(cfg)).serve(l).map_err(|e| e.to_string())
}

fn main() {
    let result = parse_args(std::env::args().skip(1).collect()).and_then(|(cmd, cfg)| match cmd.as_str() {
        "set-pin" => set_pin(&cfg),
        "check" => check(&cfg),
        _ => serve(cfg),
    });
    if let Err(e) = result {
        eprintln!("lumen-helper: {e}");
        if e.starts_with("unknown argument") {
            eprintln!("{USAGE}");
        }
        std::process::exit(1);
    }
}
