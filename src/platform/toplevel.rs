// `Windows` on labwc, through zwlr_foreign_toplevel_manager_v1 (v3). Its own
// Wayland connection: a thread dispatches events, requests go out from any
// thread. Protocol proven by examples/toplevels.rs (Job 0.3).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    self as handle_ev, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    self as mgr_ev, ZwlrForeignToplevelManagerV1,
};

use super::{Toplevel, WindowEvent, Windows};

// zwlr_foreign_toplevel_handle_v1.state enum values.
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// Decodes the `state` array (native-endian u32s) into (activated, fullscreen).
fn decode_state(state: &[u8]) -> (bool, bool) {
    let (mut activated, mut fullscreen) = (false, false);
    for c in state.chunks_exact(4) {
        match u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) {
            STATE_ACTIVATED => activated = true,
            STATE_FULLSCREEN => fullscreen = true,
            _ => {}
        }
    }
    (activated, fullscreen)
}

/// What requests and `toplevels()` see: announced windows only.
#[derive(Default)]
struct Shared {
    windows: HashMap<u64, (ZwlrForeignToplevelHandleV1, Toplevel)>,
}

/// A window the compositor has created; fields fill in until its `done`.
struct Pending {
    handle: ZwlrForeignToplevelHandleV1,
    info: Toplevel,
    announced: bool,
}

struct State {
    shared: Arc<Mutex<Shared>>,
    next_id: u64,
    /// Keyed by the handle's protocol id, which is all an event carries.
    tops: HashMap<u32, Pending>,
    on_event: Box<dyn Fn(WindowEvent) + Send>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(st: &mut Self, _: &ZwlrForeignToplevelManagerV1, ev: mgr_ev::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let mgr_ev::Event::Toplevel { toplevel } = ev {
            st.next_id += 1;
            let info = Toplevel { id: st.next_id, app_id: String::new(), title: String::new(), activated: false, fullscreen: false };
            st.tops.insert(toplevel.id().protocol_id(), Pending { handle: toplevel, info, announced: false });
        }
    }
    wayland_client::event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        mgr_ev::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(st: &mut Self, h: &ZwlrForeignToplevelHandleV1, ev: handle_ev::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let key = h.id().protocol_id();
        let Some(t) = st.tops.get_mut(&key) else { return };
        // The app's callback runs with no lock held, so it may call toplevels().
        let event = match ev {
            handle_ev::Event::Title { title } => { t.info.title = title; None }
            handle_ev::Event::AppId { app_id } => { t.info.app_id = app_id; None }
            handle_ev::Event::State { state } => {
                (t.info.activated, t.info.fullscreen) = decode_state(&state);
                None
            }
            handle_ev::Event::Done => {
                let info = t.info.clone();
                let ev = if t.announced { WindowEvent::Changed(info.clone()) } else { WindowEvent::Opened(info.clone()) };
                t.announced = true;
                st.shared.lock().unwrap().windows.insert(info.id, (t.handle.clone(), info));
                Some(ev)
            }
            handle_ev::Event::Closed => {
                let t = st.tops.remove(&key).unwrap();
                st.shared.lock().unwrap().windows.remove(&t.info.id);
                t.handle.destroy();
                // A window that closed before its first `done` was never announced.
                t.announced.then_some(WindowEvent::Closed(t.info.id))
            }
            _ => None,
        };
        if let Some(ev) = event {
            (st.on_event)(ev);
        }
    }
}

struct Labwc {
    conn: Connection,
    seat: wl_seat::WlSeat,
    shared: Arc<Mutex<Shared>>,
}

impl Labwc {
    /// Looks the handle up, sends a request on it and flushes the socket.
    fn request(&self, id: u64, what: &str, send: impl FnOnce(&ZwlrForeignToplevelHandleV1)) -> Result<(), String> {
        let handle = {
            let shared = self.shared.lock().unwrap();
            shared.windows.get(&id).map(|(h, _)| h.clone()).ok_or_else(|| format!("{what}: no window {id}"))?
        };
        send(&handle);
        self.conn.flush().map_err(|e| format!("{what}: {e}"))
    }
}

impl Windows for Labwc {
    fn toplevels(&self) -> Vec<Toplevel> {
        let mut v: Vec<_> = self.shared.lock().unwrap().windows.values().map(|(_, t)| t.clone()).collect();
        v.sort_by_key(|t| t.id);
        v
    }
    fn activate(&self, id: u64) -> Result<(), String> {
        self.request(id, "activate", |h| h.activate(&self.seat))
    }
    fn close(&self, id: u64) -> Result<(), String> {
        self.request(id, "close", |h| h.close())
    }
    fn set_fullscreen(&self, id: u64) -> Result<(), String> {
        self.request(id, "fullscreen", |h| h.set_fullscreen(None))
    }
}

/// Connects to the running compositor (WAYLAND_DISPLAY / XDG_RUNTIME_DIR) and
/// starts watching windows. Windows that already exist are reported as
/// `Opened` before this returns, on the calling thread; later events come from
/// the dispatch thread.
pub fn connect(on_event: impl Fn(WindowEvent) + Send + 'static) -> Result<Arc<dyn Windows>, String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("wayland connect: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("wayland registry: {e}"))?;
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).map_err(|e| format!("wl_seat: {e}"))?;
    let _mgr: ZwlrForeignToplevelManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .map_err(|e| format!("zwlr_foreign_toplevel_manager_v1 not offered by the compositor: {e}"))?;

    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut state = State { shared: shared.clone(), next_id: 0, tops: HashMap::new(), on_event: Box::new(on_event) };
    // One roundtrip delivers the toplevel objects, the next their details.
    for _ in 0..2 {
        queue.roundtrip(&mut state).map_err(|e| format!("wayland roundtrip: {e}"))?;
    }

    let keep_conn = conn.clone();
    std::thread::Builder::new()
        .name("windows".into())
        .spawn(move || {
            let _keep = keep_conn;
            while queue.blocking_dispatch(&mut state).is_ok() {}
            eprintln!("[windows] wayland connection closed");
        })
        .map_err(|e| format!("spawn windows thread: {e}"))?;
    Ok(Arc::new(Labwc { conn, seat, shared }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr(vals: &[u32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn decodes_state_array() {
        assert_eq!(decode_state(&[]), (false, false));
        assert_eq!(decode_state(&arr(&[2])), (true, false));
        assert_eq!(decode_state(&arr(&[3])), (false, true));
        assert_eq!(decode_state(&arr(&[0, 2, 3])), (true, true));
        // maximized (0) and minimized (1) alone change neither flag
        assert_eq!(decode_state(&arr(&[0, 1])), (false, false));
        // a trailing partial word is ignored
        assert_eq!(decode_state(&[2, 0, 0]), (false, false));
    }
}
