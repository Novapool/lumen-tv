// Probe for zwlr_foreign_toplevel_manager_v1 (labwc): list, watch and control
// other apps' windows. Run on the Pi with XDG_RUNTIME_DIR and WAYLAND_DISPLAY set:
//
//   toplevels list
//   toplevels watch
//   toplevels activate|close|fullscreen <app_id>
//   toplevels wait-activated <app_id> [timeout_ms]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("toplevels needs Linux (Wayland)");
}

#[cfg(target_os = "linux")]
fn main() {
    tl::run();
}

#[cfg(target_os = "linux")]
mod tl {
    use std::collections::HashMap;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use wayland_client::protocol::{wl_registry, wl_seat};
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
        self as handle_ev, ZwlrForeignToplevelHandleV1,
    };
    use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
        self as mgr_ev, ZwlrForeignToplevelManagerV1,
    };

    fn now_ms() -> u128 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
    }

    struct Top {
        handle: ZwlrForeignToplevelHandleV1,
        app_id: String,
        title: String,
        activated: bool,
        fullscreen: bool,
        minimized: bool,
        announced: bool,
    }

    #[derive(Default)]
    struct App {
        tops: HashMap<u32, Top>,
        watch: bool,
        closed_log: bool,
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
        fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }
    impl Dispatch<wl_seat::WlSeat, ()> for App {
        fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for App {
        fn event(app: &mut Self, _: &ZwlrForeignToplevelManagerV1, ev: mgr_ev::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            if let mgr_ev::Event::Toplevel { toplevel } = ev {
                let id = toplevel.id().protocol_id();
                app.tops.insert(
                    id,
                    Top { handle: toplevel, app_id: String::new(), title: String::new(), activated: false, fullscreen: false, minimized: false, announced: false },
                );
            }
        }
        wayland_client::event_created_child!(App, ZwlrForeignToplevelManagerV1, [
            mgr_ev::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
        ]);
    }

    impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for App {
        fn event(app: &mut Self, h: &ZwlrForeignToplevelHandleV1, ev: handle_ev::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            let id = h.id().protocol_id();
            let watch = app.watch;
            let Some(t) = app.tops.get_mut(&id) else { return };
            match ev {
                handle_ev::Event::Title { title } => t.title = title,
                handle_ev::Event::AppId { app_id } => t.app_id = app_id,
                handle_ev::Event::State { state } => {
                    t.activated = false;
                    t.fullscreen = false;
                    t.minimized = false;
                    for c in state.chunks_exact(4) {
                        match u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) {
                            1 => t.minimized = true,
                            2 => t.activated = true,
                            3 => t.fullscreen = true,
                            _ => {}
                        }
                    }
                }
                handle_ev::Event::Done => {
                    if watch {
                        let kind = if t.announced { "changed" } else { "opened" };
                        println!("{} {kind} {} activated={} fullscreen={} title={:?}", now_ms(), t.app_id, t.activated, t.fullscreen, t.title);
                    }
                    t.announced = true;
                }
                handle_ev::Event::Closed => {
                    if watch {
                        println!("{} closed {} activated={}", now_ms(), t.app_id, t.activated);
                    }
                    app.closed_log = true;
                    app.tops.remove(&id);
                }
                _ => {}
            }
        }
    }

    fn print_list(app: &App) {
        let mut v: Vec<_> = app.tops.iter().collect();
        v.sort_by_key(|(id, _)| **id);
        for (id, t) in v {
            println!("{id} app_id={:?} title={:?} activated={} fullscreen={} minimized={}", t.app_id, t.title, t.activated, t.fullscreen, t.minimized);
        }
    }

    pub fn run() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cmd = args.first().map(String::as_str).unwrap_or("");
        let target = args.get(1).cloned().unwrap_or_default();
        if !matches!(cmd, "list" | "watch" | "activate" | "close" | "fullscreen" | "wait-activated") {
            eprintln!("usage: toplevels list | watch | activate|close|fullscreen <app_id> | wait-activated <app_id> [timeout_ms]");
            std::process::exit(2);
        }

        let conn = Connection::connect_to_env().expect("connect to Wayland (XDG_RUNTIME_DIR / WAYLAND_DISPLAY)");
        let (globals, mut queue) = registry_queue_init::<App>(&conn).expect("registry");
        let qh = queue.handle();
        for g in globals.contents().clone_list() {
            if g.interface == "zwlr_foreign_toplevel_manager_v1" {
                eprintln!("advertised: zwlr_foreign_toplevel_manager_v1 version {}", g.version);
            }
        }
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).expect("wl_seat");
        let _mgr: ZwlrForeignToplevelManagerV1 = globals.bind(&qh, 1..=3, ()).expect("zwlr_foreign_toplevel_manager_v1 not advertised");

        let mut app = App { watch: cmd == "watch" || cmd == "wait-activated", ..Default::default() };
        // First roundtrip: toplevel objects; second: their title/app_id/state/done.
        queue.roundtrip(&mut app).unwrap();
        queue.roundtrip(&mut app).unwrap();

        let find = |app: &App| app.tops.values().find(|t| t.app_id == target).map(|t| t.handle.clone());
        match cmd {
            "list" => print_list(&app),
            "watch" => loop {
                queue.blocking_dispatch(&mut app).expect("dispatch");
            },
            "activate" | "close" | "fullscreen" => {
                let Some(h) = find(&app) else {
                    eprintln!("no toplevel with app_id {target:?}");
                    print_list(&app);
                    std::process::exit(1);
                };
                println!("{} {cmd} {target}", now_ms());
                match cmd {
                    "activate" => h.activate(&seat),
                    "close" => h.close(),
                    _ => h.set_fullscreen(None),
                }
                queue.roundtrip(&mut app).unwrap();
                queue.roundtrip(&mut app).unwrap();
                print_list(&app);
            }
            _ => {
                // wait-activated
                let timeout: u64 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(10_000);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(timeout));
                    eprintln!("timeout waiting for {:?}", std::env::args().nth(2));
                    std::process::exit(1);
                });
                loop {
                    if app.tops.values().any(|t| t.app_id == target && t.activated) {
                        println!("{} activated {target}", now_ms());
                        return;
                    }
                    queue.blocking_dispatch(&mut app).expect("dispatch");
                }
            }
        }
    }
}
