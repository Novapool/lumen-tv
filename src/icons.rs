// Icons for the app tiles: find the file a registry `icon` value names, decode
// it once at startup (SVG tree or PNG pixels), and hand it to bake::tile_face,
// which draws it into the tile's face image on the CPU. Nothing here runs per
// frame, and no brand artwork ships in the repo: names resolve against the
// system's hicolor theme at runtime.

use std::fmt;
use std::path::{Path, PathBuf};

use resvg::{tiny_skia, usvg};

/// Where bare icon names are looked up. Pass temp dirs in tests.
#[derive(Clone, Debug)]
pub struct Roots {
    /// Icon theme roots, each holding `hicolor/…`, in lookup order.
    pub icons: Vec<PathBuf>,
    /// Flat pixmap dirs (`<name>.svg|png`), tried last.
    pub pixmaps: Vec<PathBuf>,
}

impl Roots {
    /// flatpak user, flatpak system, ~/.local/share/icons, /usr/share/icons, then /usr/share/pixmaps.
    pub fn system() -> Roots {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        Roots {
            icons: vec![
                data_home.join("flatpak/exports/share/icons"),
                PathBuf::from("/var/lib/flatpak/exports/share/icons"),
                home.join(".local/share/icons"),
                PathBuf::from("/usr/share/icons"),
            ],
            pixmaps: vec![PathBuf::from("/usr/share/pixmaps")],
        }
    }
}

/// A decoded icon.
pub enum Icon {
    Svg(usvg::Tree),
    /// Premultiplied RGBA8.
    Raster { w: u32, h: u32, rgba: Vec<u8> },
}

impl fmt::Debug for Icon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Icon::Svg(t) => write!(f, "svg {}x{}", t.size().width(), t.size().height()),
            Icon::Raster { w, h, .. } => write!(f, "png {w}x{h}"),
        }
    }
}

impl Icon {
    /// Natural size in pixels (SVG: its declared size).
    pub fn size(&self) -> (f32, f32) {
        match self {
            Icon::Svg(t) => (t.size().width(), t.size().height()),
            Icon::Raster { w, h, .. } => (*w as f32, *h as f32),
        }
    }
}

/// Outcome of resolving and decoding one registry entry's icon (kept for M3.5).
#[allow(dead_code)] // the path / reason are for Settings
#[derive(Debug)]
pub enum IconStatus {
    Ready(Icon, PathBuf),
    Missing(String),
}

fn has_image_ext(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.ends_with(".svg") || l.ends_with(".png")
}

/// Finds the file `spec` names: an absolute path; a path relative to
/// `config_dir` (contains `/` or ends .svg/.png); or a bare hicolor theme name
/// (which may contain dots, like `org.jellyfin.JellyfinDesktop`).
pub fn resolve(spec: &str, config_dir: &Path, roots: &Roots) -> Result<PathBuf, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty icon".into());
    }
    let must_exist = |p: PathBuf| if p.is_file() { Ok(p) } else { Err(format!("no such file {}", p.display())) };
    if Path::new(spec).is_absolute() {
        return must_exist(PathBuf::from(spec));
    }
    if spec.contains('/') || has_image_ext(spec) {
        return must_exist(config_dir.join(spec));
    }
    for root in &roots.icons {
        let hicolor = root.join("hicolor");
        let svg = hicolor.join("scalable/apps").join(format!("{spec}.svg"));
        if svg.is_file() {
            return Ok(svg);
        }
        if let Some(png) = largest_png(&hicolor, spec) {
            return Ok(png);
        }
    }
    for dir in &roots.pixmaps {
        for ext in ["svg", "png"] {
            let p = dir.join(format!("{spec}.{ext}"));
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(format!("icon '{spec}' not found in the hicolor theme or pixmaps"))
}

/// `hicolor/<N>x<N>/apps/<name>.png` with the biggest N.
fn largest_png(hicolor: &Path, name: &str) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for entry in std::fs::read_dir(hicolor).ok()?.flatten() {
        let dir = entry.file_name().to_string_lossy().into_owned();
        let Some((a, b)) = dir.split_once('x') else { continue };
        let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) else { continue };
        if a != b {
            continue;
        }
        let p = entry.path().join("apps").join(format!("{name}.png"));
        if p.is_file() && best.as_ref().is_none_or(|(n, _)| a > *n) {
            best = Some((a, p));
        }
    }
    best.map(|(_, p)| p)
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Decodes an SVG or PNG file. Never panics on bad input.
pub fn load(path: &Path) -> Result<Icon, String> {
    let data = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if data.starts_with(PNG_MAGIC) {
        let pm = tiny_skia::Pixmap::decode_png(&data).map_err(|e| format!("bad PNG: {e}"))?;
        if pm.width() == 0 || pm.height() == 0 {
            return Err("empty PNG".into());
        }
        return Ok(Icon::Raster { w: pm.width(), h: pm.height(), rgba: pm.take() });
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if ext == "svg" || ext == "svgz" {
        let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).map_err(|e| format!("bad SVG: {e}"))?;
        let s = tree.size();
        if !(s.width() > 0. && s.height() > 0.) {
            return Err("SVG has no size".into());
        }
        return Ok(Icon::Svg(tree));
    }
    Err(format!("unknown image format ({})", path.display()))
}

/// Resolves and decodes one registry entry's icon and logs one `[icons]` line.
pub fn for_entry(id: &str, spec: &str, config_dir: &Path, roots: &Roots) -> IconStatus {
    match resolve(spec, config_dir, roots).and_then(|p| load(&p).map(|i| (i, p))) {
        Ok((icon, path)) => {
            println!("[icons] {id}: {} ({icon:?})", path.display());
            IconStatus::Ready(icon, path)
        }
        Err(why) => {
            println!("[icons] {id}: {why}; keeping the glyph");
            IconStatus::Missing(why)
        }
    }
}

/// Renders `icon` at exactly tw x th pixels as premultiplied RGBA8.
/// SVG is rasterised at that size; a raster is area-averaged (box filter).
pub fn render(icon: &Icon, tw: u32, th: u32) -> Vec<u8> {
    match icon {
        Icon::Svg(tree) => {
            let Some(mut pm) = tiny_skia::Pixmap::new(tw, th) else { return vec![0; (tw * th * 4) as usize] };
            let s = tree.size();
            let t = tiny_skia::Transform::from_scale(tw as f32 / s.width(), th as f32 / s.height());
            resvg::render(tree, t, &mut pm.as_mut());
            pm.take()
        }
        Icon::Raster { w, h, rgba } => area_average(rgba, *w, *h, tw, th),
    }
}

/// Box-filter resample of premultiplied RGBA8 (each destination pixel averages
/// the source area it covers, with fractional edge weights).
fn area_average(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh {
        let (y0, y1) = (dy as f32 * fy, (dy + 1) as f32 * fy);
        for dx in 0..dw {
            let (x0, x1) = (dx as f32 * fx, (dx + 1) as f32 * fx);
            let mut acc = [0f32; 4];
            let mut wsum = 0f32;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(sh) {
                let wy = (sy as f32 + 1.).min(y1) - (sy as f32).max(y0);
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(sw) {
                    let wgt = wy * ((sx as f32 + 1.).min(x1) - (sx as f32).max(x0));
                    let i = ((sy * sw + sx) * 4) as usize;
                    for k in 0..4 {
                        acc[k] += src[i + k] as f32 * wgt;
                    }
                    wsum += wgt;
                }
            }
            let o = ((dy * dw + dx) * 4) as usize;
            if wsum > 0. {
                for k in 0..4 {
                    out[o + k] = (acc[k] / wsum).round().clamp(0., 255.) as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;

    pub fn tmp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!("lumen-icons-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    pub const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32"><rect width="64" height="32" fill="#f00"/></svg>"##;

    pub fn png(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
        pm.encode_png().unwrap()
    }

    fn put(p: &Path, data: impl AsRef<[u8]>) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    fn roots(d: &Path) -> Roots {
        Roots { icons: vec![d.join("a"), d.join("b")], pixmaps: vec![d.join("pix")] }
    }

    #[test]
    fn absolute_path_used_as_is() {
        let d = tmp("abs");
        let f = d.join("x.png");
        put(&f, png(4, 4, [1, 2, 3, 255]));
        assert_eq!(resolve(f.to_str().unwrap(), &d, &roots(&d)).unwrap(), f);
        assert!(resolve(d.join("nope.png").to_str().unwrap(), &d, &roots(&d)).is_err());
    }

    #[test]
    fn relative_path_is_under_config_dir() {
        let d = tmp("rel");
        let cfg = d.join("cfg");
        put(&cfg.join("icons/j.png"), png(4, 4, [1, 2, 3, 255]));
        assert_eq!(resolve("icons/j.png", &cfg, &roots(&d)).unwrap(), cfg.join("icons/j.png"));
        put(&cfg.join("k.svg"), SVG);
        assert!(resolve("k.svg", &cfg, &roots(&d)).is_ok());
        assert!(resolve("missing.png", &cfg, &roots(&d)).is_err());
    }

    #[test]
    fn bare_name_with_dots_is_a_theme_name() {
        let d = tmp("dots");
        put(&d.join("a/hicolor/256x256/apps/org.x.App.png"), png(4, 4, [0, 0, 0, 255]));
        let p = resolve("org.x.App", &d, &roots(&d)).unwrap();
        assert!(p.ends_with("256x256/apps/org.x.App.png"));
    }

    #[test]
    fn svg_preferred_over_png() {
        let d = tmp("pref");
        put(&d.join("a/hicolor/256x256/apps/foo.png"), png(4, 4, [0, 0, 0, 255]));
        put(&d.join("a/hicolor/scalable/apps/foo.svg"), SVG);
        assert!(resolve("foo", &d, &roots(&d)).unwrap().ends_with("scalable/apps/foo.svg"));
    }

    #[test]
    fn largest_png_wins() {
        let d = tmp("big");
        for n in [48, 256, 64, 128] {
            put(&d.join(format!("a/hicolor/{n}x{n}/apps/foo.png")), png(4, 4, [0, 0, 0, 255]));
        }
        put(&d.join("a/hicolor/512x256/apps/foo.png"), png(4, 4, [0, 0, 0, 255])); // not square: ignored
        assert!(resolve("foo", &d, &roots(&d)).unwrap().ends_with("256x256/apps/foo.png"));
    }

    #[test]
    fn root_order_then_pixmaps() {
        let d = tmp("order");
        put(&d.join("b/hicolor/64x64/apps/foo.png"), png(4, 4, [0, 0, 0, 255]));
        put(&d.join("pix/foo.png"), png(4, 4, [0, 0, 0, 255]));
        put(&d.join("pix/only.png"), png(4, 4, [0, 0, 0, 255]));
        assert!(resolve("foo", &d, &roots(&d)).unwrap().starts_with(d.join("b")));
        assert_eq!(resolve("only", &d, &roots(&d)).unwrap(), d.join("pix/only.png"));
        put(&d.join("a/hicolor/16x16/apps/foo.png"), png(4, 4, [0, 0, 0, 255]));
        assert!(resolve("foo", &d, &roots(&d)).unwrap().starts_with(d.join("a")));
    }

    #[test]
    fn missing_name_is_an_error() {
        let d = tmp("miss");
        assert!(resolve("plex", &d, &roots(&d)).is_err());
        assert!(resolve("", &d, &roots(&d)).is_err());
        assert!(matches!(for_entry("plex", "plex", &d, &roots(&d)), IconStatus::Missing(_)));
    }

    #[test]
    fn load_png_and_svg() {
        let d = tmp("load");
        put(&d.join("a.png"), png(8, 4, [255, 0, 0, 255]));
        put(&d.join("a.svg"), SVG);
        assert!(matches!(load(&d.join("a.png")).unwrap(), Icon::Raster { w: 8, h: 4, .. }));
        assert_eq!(load(&d.join("a.svg")).unwrap().size(), (64., 32.));
    }

    #[test]
    fn corrupt_files_are_errors() {
        let d = tmp("bad");
        put(&d.join("a.png"), b"\x89PNG\r\n\x1a\ngarbage");
        put(&d.join("b.svg"), b"<svg this is not xml");
        put(&d.join("c.png"), b"not an image at all");
        put(&d.join("d.bin"), b"whatever");
        for f in ["a.png", "b.svg", "c.png", "d.bin", "missing.png"] {
            assert!(load(&d.join(f)).is_err(), "{f}");
        }
    }

    #[test]
    fn render_svg_exact_size_and_png_area_average() {
        let d = tmp("render");
        put(&d.join("a.svg"), SVG);
        let svg = load(&d.join("a.svg")).unwrap();
        let px = render(&svg, 16, 8);
        assert_eq!(px.len(), 16 * 8 * 4);
        assert_eq!(&px[..4], &[255, 0, 0, 255]);

        // 2x2 checker of black/white downscaled to 1x1 averages to mid grey
        let src = vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255];
        assert_eq!(area_average(&src, 2, 2, 1, 1), vec![128, 128, 128, 255]);
        // same size is the identity
        assert_eq!(area_average(&src, 2, 2, 2, 2), src);
    }

    /// Run on the Pi: `cargo test --release -- --ignored --nocapture resolve_probe`.
    #[test]
    #[ignore]
    fn resolve_probe() {
        let roots = Roots::system();
        for name in ["kodi", "steamlink", "org.jellyfin.JellyfinDesktop", "chromium", "plex"] {
            println!("PROBE {name}: {:?}", resolve(name, &crate::registry::config_dir(), &roots));
            let _ = for_entry(name, name, &crate::registry::config_dir(), &roots);
        }
    }
}
