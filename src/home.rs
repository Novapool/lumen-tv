// The home screen's tiles: built from the registry at startup and rebuilt by
// `reload` (after a store install/uninstall) without restarting. The Slint
// `apps` model holds what the UI draws; `Home` (here, in a UI-thread
// `thread_local!`) holds what it doesn't: each tile's gradient, icon outcome,
// the registry's load errors and the last tile size. Row i of the model is
// row i of `Home`. UI thread only.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{Model, VecModel};

use crate::{bake, icons, registry, App, AppWindow};

/// What the home screen keeps beyond the Slint model, by row i of the model:
/// each tile's gradient (bake input) and icon outcome (`None` for the built-in
/// tiles), plus the registry's load errors and the last `rem` baked at.
#[derive(Default)]
pub struct Home {
    tints: Vec<bake::Tint>,
    pub errors: Vec<registry::LoadError>,
    pub icons: Vec<Option<icons::IconStatus>>,
    /// Physical pixels per rem of the last `rebake-tiles` (0 until the window is sized).
    rem: f32,
}

thread_local! {
    static HOME: RefCell<Home> = RefCell::new(Home::default());
}

/// Reads the current `Home` (Settings). Don't call UI setters inside `f`.
pub fn with<R>(f: impl FnOnce(&Home) -> R) -> R {
    HOME.with(|h| f(&h.borrow()))
}

/// What a reload changed, by registry id (built-in tiles never change).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reloaded {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// `#rrggbb` (validated by the registry) to rgb.
fn hex_rgb(s: &str) -> [u8; 3] {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// The 135 degree tile gradient: the stops spread evenly.
fn tile_tint(colors: Option<&[String]>) -> bake::Tint {
    let default = ["#24262c".to_string(), "#6b707c".to_string()];
    let rgb: Vec<[u8; 3]> = colors.unwrap_or(&default).iter().map(|c| hex_rgb(c)).collect();
    bake::Tint::from_stops(135., &rgb)
}

/// The built-in tiles after the registry's: id, name, glyph, gradient.
const BUILT_IN: [(&str, &str, &str, &str, &str); 3] = [
    ("lumen:store", "Store", "+", "#003b3b", "#19c3b0"),
    ("lumen:settings", "Settings", "⚙", "#24262c", "#6b707c"),
    ("lumen:power", "Power", "⏻", "#3a0d12", "#8a2a33"),
];

/// Decodes the icons and builds the model rows (registry entries, then Store,
/// Settings and Power) with their heroes, and faces too when `rem` is known.
fn build_rows(loaded: &registry::Loaded, rem: f32, running: &dyn Fn(&str) -> bool) -> (Vec<App>, Home) {
    let (config, roots) = (registry::config_dir(), icons::Roots::system());
    let mut apps = Vec::new();
    let mut home = Home { rem, errors: loaded.errors.clone(), ..Default::default() };
    for e in &loaded.entries {
        let status = e.icon.as_deref().map(|spec| icons::for_entry(&e.id, spec, &config, &roots));
        let ready = matches!(status, Some(icons::IconStatus::Ready(..)));
        let glyph = e.glyph.clone().unwrap_or_else(|| e.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
        apps.push(App {
            id: e.id.clone().into(),
            running: running(&e.id),
            name: e.name.clone().into(),
            glyph: if ready { "".into() } else { glyph.into() },
            hero: Default::default(),
            face: Default::default(),
        });
        home.tints.push(tile_tint(e.colors.as_deref()));
        home.icons.push(status);
    }
    for (id, name, glyph, c0, c1) in BUILT_IN {
        apps.push(App {
            id: id.into(),
            running: false,
            name: name.into(),
            glyph: glyph.into(),
            hero: Default::default(),
            face: Default::default(),
        });
        home.tints.push(tile_tint(Some(&[c0.to_string(), c1.to_string()])));
        home.icons.push(None);
    }
    let t = std::time::Instant::now();
    for (i, app) in apps.iter_mut().enumerate() {
        app.hero = bake::hero(&home.tints[i]);
        if rem >= 1. {
            app.face = face(&home, i, rem);
        }
    }
    println!("[bake] heroes{} in {} ms", if rem >= 1. { " + faces" } else { "" }, t.elapsed().as_millis());
    (apps, home)
}

fn face(home: &Home, i: usize, rem: f32) -> slint::Image {
    let icon = match home.icons.get(i) {
        Some(Some(icons::IconStatus::Ready(icon, _))) => Some(icon),
        _ => None,
    };
    bake::tile_face(&home.tints[i], rem, icon)
}

fn log_tiles(ui: &AppWindow) {
    let names: Vec<String> = ui.get_apps().iter().map(|a| a.name.to_string()).collect();
    println!("[home] tiles: {}", names.join(", "));
}

/// Loads the registry (seeding the defaults on first start), builds the tiles
/// and bakes their heroes. Returns the entries for the lifecycle.
pub fn init(ui: &AppWindow) -> Vec<registry::Entry> {
    let dir = registry::apps_dir();
    match registry::seed_defaults(&dir) {
        Ok(true) => println!("[registry] wrote the default apps to {}", dir.display()),
        Ok(false) => {}
        Err(e) => eprintln!("[registry] could not seed {}: {e}", dir.display()),
    }
    let loaded = registry::load(&dir);
    let (apps, home) = build_rows(&loaded, 0., &|_| false);
    HOME.with(|h| *h.borrow_mut() = home);
    ui.set_apps(Rc::new(VecModel::from(apps)).into());
    log_tiles(ui);
    loaded.entries
}

/// Bakes the tile faces and the shared shadow for `rem` physical pixels per
/// rem, and remembers `rem` for later reloads.
pub fn bake_tiles(ui: &AppWindow, rem: f32) {
    if rem < 1. {
        return; // window not sized yet
    }
    let t = std::time::Instant::now();
    let faces: Vec<slint::Image> = HOME.with(|h| {
        let mut h = h.borrow_mut();
        h.rem = rem;
        (0..h.tints.len()).map(|i| face(&h, i, rem)).collect()
    });
    let apps = ui.get_apps();
    for (i, face) in faces.into_iter().enumerate() {
        if let Some(app) = apps.row_data(i) {
            apps.set_row_data(i, App { face, ..app });
        }
    }
    ui.set_tile_shadow(bake::tile_shadow(rem));
    println!("[bake] tiles at {rem} px/rem in {} ms", t.elapsed().as_millis());
}

/// Where focus lands after the tiles were rebuilt: on the same tile id if it
/// still exists, else the old position clamped to the new row.
pub fn focus_after_reload(old_id: Option<&str>, old_focused: usize, new_ids: &[&str]) -> usize {
    old_id
        .and_then(|id| new_ids.iter().position(|n| *n == id))
        .unwrap_or_else(|| old_focused.min(new_ids.len().saturating_sub(1)))
}

/// Re-reads the registry and rebuilds the tiles and the lifecycle's specs
/// without a restart. Focus stays on the same tile id if it still exists
/// (else it is clamped), and the running dots carry over. UI thread only.
pub fn reload(ui: &AppWindow) -> Reloaded {
    let loaded = registry::load(&registry::apps_dir());
    let old = ui.get_apps();
    let old_rows: Vec<App> = old.iter().collect();
    let focused_id = old_rows.get(ui.get_focused().max(0) as usize).map(|a| a.id.clone());
    let rem = HOME.with(|h| h.borrow().rem);

    let (apps, home) = build_rows(&loaded, rem, &|id| old_rows.iter().any(|a| a.running && a.id == id));
    let reg_ids = |rows: &[App]| -> Vec<String> {
        rows.iter().map(|a| a.id.to_string()).filter(|id| !id.starts_with("lumen:") && !id.is_empty()).collect()
    };
    let (before, after) = (reg_ids(&old_rows), reg_ids(&apps));
    let out = Reloaded {
        added: after.iter().filter(|id| !before.contains(id)).cloned().collect(),
        removed: before.iter().filter(|id| !after.contains(id)).cloned().collect(),
    };

    let new_ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    let focus = focus_after_reload(focused_id.as_deref(), ui.get_focused().max(0) as usize, &new_ids);
    HOME.with(|h| *h.borrow_mut() = home);
    ui.set_apps(Rc::new(VecModel::from(apps)).into());
    ui.set_focused(focus as i32);

    let list = |sign: char, ids: &[String]| ids.iter().map(|id| format!("{sign}{id}")).collect::<Vec<_>>();
    let changes = [list('+', &out.added), list('-', &out.removed)].concat().join(", ");
    println!(
        "[registry] reloaded: {} apps ({changes}), {} errors",
        loaded.entries.len(),
        loaded.errors.len()
    );
    log_tiles(ui);
    #[cfg(target_os = "linux")]
    crate::session::set_entries(loaded.entries);
    out
}

#[cfg(test)]
mod tests {
    use super::focus_after_reload;

    #[test]
    fn focus_follows_the_tile_id() {
        // A tile was added before the focused one: focus moves with it.
        assert_eq!(focus_after_reload(Some("b"), 1, &["x", "a", "b", "lumen:store"]), 2);
        // A tile before it was removed.
        assert_eq!(focus_after_reload(Some("b"), 2, &["b", "lumen:store"]), 0);
    }

    #[test]
    fn focus_is_clamped_when_the_tile_is_gone() {
        // The focused tile was uninstalled: keep the position, clamped to the row.
        assert_eq!(focus_after_reload(Some("gone"), 2, &["a", "b", "c", "d"]), 2);
        assert_eq!(focus_after_reload(Some("gone"), 5, &["a", "lumen:store"]), 1);
        assert_eq!(focus_after_reload(None, 3, &["a", "b"]), 1);
        assert_eq!(focus_after_reload(Some("a"), 0, &[]), 0);
    }
}
