// Lumen backend. M1: just hosts the UI and gives the perf benchmark a way to
// report results to the journal. Input, registry, store and platform modules
// arrive in M2-M4.

/// True when the session asked for a startup benchmark (LUMEN_BENCH=1 in
/// ~/.config/lumen-tv/env, which the systemd unit loads).
#[tauri::command]
fn bench_requested() -> bool {
    std::env::var("LUMEN_BENCH").is_ok_and(|v| v == "1")
}

/// Frame-time results from the UI. stdout goes to the journal, so
/// `journalctl -u lumen-test -g perf` shows them.
#[tauri::command]
fn perf_report(report: String) {
    println!("[perf] {report}");
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![bench_requested, perf_report, quit])
        .run(tauri::generate_context!())
        .expect("error while running Lumen");
}
