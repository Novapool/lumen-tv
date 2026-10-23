use std::process::Command;

fn main() {
    slint_build::compile("ui/app.slint").unwrap();

    // Build info for the About panel. LUMEN_GIT wins (scripts/pi-build.sh sets
    // it because .git is not synced to the Pi).
    println!("cargo:rerun-if-env-changed=LUMEN_GIT");
    // Re-run on a new commit (HEAD, or the branch ref it points to). Missing
    // paths (no .git on the Pi) just mean cargo reruns every build, which is fine there.
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        println!("cargo:rerun-if-changed=.git/HEAD");
        if let Some(r) = head.trim().strip_prefix("ref: ") {
            println!("cargo:rerun-if-changed=.git/{r}");
        }
    }
    let git = std::env::var("LUMEN_GIT").ok().filter(|g| !g.is_empty()).or_else(|| {
        let out = Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|g| !g.is_empty())
    });
    println!("cargo:rustc-env=LUMEN_GIT={}", git.unwrap_or_else(|| "unknown".into()));
    let date = Command::new("date").arg("+%Y-%m-%d").output().ok().filter(|o| o.status.success());
    let date = date.map_or_else(|| "unknown".into(), |o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    println!("cargo:rustc-env=LUMEN_BUILD_DATE={date}");
}
