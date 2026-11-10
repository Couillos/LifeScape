//! Precomputed spectator map: whole painted world as static PNGs.
//!
//! The live `/minimap.png` route rendered a 33-tile window per request
//! behind a global lock; one moving bot means one new origin per step and
//! the dashboard refetched constantly. Instead the daemon renders the
//! whole world once at startup (4 px per tile, walls and mapscene sprites
//! included, same passes as a live frame) into 16 PNGs, served immutable;
//! the client only crops around the player. Regeneration happens only
//! when the pack zip hash changes, otherwise startup skips everything.

use std::fmt;
use std::path::Path;
use std::time::Instant;

use lifescape_json::{self as json, Value, object, str_value};

use crate::mapcache;
use crate::minimap::{Minimap, pack_zip};
use crate::png;

struct Cluster {
    name: &'static str,
    zlo: i32,
    zhi: i32,
    levels: &'static [i32],
}

/// Painted square-z bands from the pack (3493 of the 3976 bbox squares are
/// empty ocean): one file per band per non-empty level, 16 files total.
const CLUSTERS: [Cluster; 5] = [
    Cluster { name: "i20", zlo: 20, zhi: 20, levels: &[0] },
    Cluster { name: "c44", zlo: 44, zhi: 62, levels: &[0, 1, 2, 3] },
    Cluster { name: "c70", zlo: 70, zhi: 77, levels: &[0, 1, 2, 3] },
    Cluster { name: "c144", zlo: 144, zhi: 158, levels: &[0, 1, 2, 3] },
    Cluster { name: "c160", zlo: 160, zhi: 161, levels: &[0, 1, 2] },
];

/// Tile margin around each cluster: a player on the painted edge still
/// crops a full 33-tile window (R16) from the file, and sprites from edge
/// locs keep their overflow room.
const MARGIN_TILES: i32 = 16;
/// Client window side in tiles (R16): crops are always this size.
pub(crate) const WINDOW_TILES: i32 = 33;

/// One static image: tile bbox plus level, in 4 px-per-tile pixels.
pub(crate) struct ImageSpec {
    pub file: String,
    pub level: i32,
    pub x0: i32,
    pub z0: i32,
    pub w: usize,
    pub h: usize,
}

/// File bboxes from the squares the pack ships: a band with no squares
/// yields no files (fallback text client-side, never a partial map).
pub(crate) fn plan_images(squares: &[(i32, i32)]) -> Vec<ImageSpec> {
    let mut specs = Vec::new();
    for cluster in &CLUSTERS {
        let mut xs: Vec<i32> = squares
            .iter()
            .filter(|(_, square_z)| (cluster.zlo..=cluster.zhi).contains(square_z))
            .map(|(square_x, _)| *square_x)
            .collect();
        if xs.is_empty() {
            continue;
        }
        xs.sort();
        let (xlo, xhi) = (xs[0], xs[xs.len() - 1]);
        let (x0, z0) = (xlo * 64 - MARGIN_TILES, cluster.zlo * 64 - MARGIN_TILES);
        let (w, h) =
            (((xhi - xlo + 1) * 64 + MARGIN_TILES * 2) as usize, ((cluster.zhi - cluster.zlo + 1) * 64 + MARGIN_TILES * 2) as usize);
        for level in cluster.levels {
            specs.push(ImageSpec {
                file: format!("{}-l{level}.png", cluster.name),
                level: *level,
                x0,
                z0,
                w,
                h,
            });
        }
    }
    specs
}

/// Source pixel rect of a 33-tile window at (ox, oz) inside a file, or
/// None when the window is not fully covered. Row 0 is the northern edge,
/// like the renderer: the window top tile is oz + 32.
#[cfg(test)]
pub(crate) fn source_rect(spec: &ImageSpec, ox: i32, oz: i32) -> Option<(i32, i32)> {
    if ox < spec.x0 || oz < spec.z0 {
        return None;
    }
    if ox + WINDOW_TILES > spec.x0 + spec.w as i32 || oz + WINDOW_TILES > spec.z0 + spec.h as i32 {
        return None;
    }
    Some(((ox - spec.x0) * 4, (spec.z0 + spec.h as i32 - 1 - (oz + WINDOW_TILES - 1)) * 4))
}

fn fnv_hex(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn dir_name(hash: &str) -> String {
    format!("map-{hash}")
}

/// True unless the stamp matches this pack and every manifest file is on
/// disk: the check reads only the stamp and the manifest, never the pack.
fn needs_rebuild(static_dir: &Path, hash: &str) -> bool {
    let dir = static_dir.join(dir_name(hash));
    let stamp = static_dir.join(format!("{}.stamp", dir_name(hash)));
    if std::fs::read_to_string(&stamp).map(|text| text.trim().to_string()).unwrap_or_default() != hash {
        return true;
    }
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap_or_default();
    let parsed = json::parse(&manifest).unwrap_or(Value::Null);
    let files = parsed.get("images").as_arr().map(|items| items.to_vec()).unwrap_or_default();
    if files.is_empty() {
        return true;
    }
    files.iter().all(|image| {
        let file = image.get("file").as_str().unwrap_or("");
        !file.is_empty() && dir.join(file).is_file()
    }) == false
}

/// Drop every `map-*` generation except the current one and the newest
/// previous complete pair (dir plus stamp); tmp leftovers always go.
fn purge_old(static_dir: &Path, keep_hash: &str) {
    let entries = std::fs::read_dir(static_dir).map(|entries| entries.flatten().collect::<Vec<_>>()).unwrap_or_default();
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".tmp") && name.starts_with("map-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let mut hashes: Vec<(String, std::time::SystemTime)> = Vec::new();
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(hash) = name.strip_prefix("map-").and_then(|rest| rest.strip_suffix(".stamp")) else {
            continue;
        };
        if hash == keep_hash || hash.len() != 16 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let dir = static_dir.join(dir_name(hash));
        if dir.is_dir() && !hashes.iter().any(|(known, _)| known == hash) {
            let mtime = std::fs::metadata(&dir).and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH);
            hashes.push((hash.to_string(), mtime));
        }
    }
    // The stamp marks a complete pair: without one the dir is a crashed
    // build, not a previous generation.
    hashes.sort_by(|a, b| b.1.cmp(&a.1));
    for (hash, _) in hashes.into_iter().skip(1) {
        let _ = std::fs::remove_dir_all(static_dir.join(dir_name(&hash)));
        let _ = std::fs::remove_file(static_dir.join(format!("{}.stamp", dir_name(&hash))));
    }
    // Incomplete leftovers (dir without stamp) never serve: drop them.
    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(hash) = name.strip_prefix("map-") else {
            continue;
        };
        if hash == keep_hash || hash.len() != 16 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if entry.path().is_dir() && !static_dir.join(format!("map-{hash}.stamp")).is_file() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Render every planned image into `out_tmp` (plus its manifest); pure
/// build step, no rename, no stamp, so tests can point it at a temp dir.
pub(crate) fn build_all(maps: &Minimap, out_tmp: &Path, hash: &str) -> Result<Vec<(String, u64)>, String> {
    let specs = plan_images(&maps.map_squares());
    if specs.is_empty() {
        return Err("pack ships no map squares".to_string());
    }
    std::fs::create_dir_all(out_tmp).map_err(|error| error.to_string())?;
    let timing = std::env::var("MAPBUILD_TIMING").is_ok();
    let mut files = Vec::new();
    for spec in &specs {
        let mark = std::time::Instant::now();
        let pixels = maps.render_static(spec.level, spec.x0, spec.z0, spec.w, spec.h);
        if pixels.iter().all(|pixel| *pixel == 0) {
            continue;
        }
        let mut rgb = Vec::with_capacity(pixels.len() * 3);
        for pixel in pixels {
            rgb.extend_from_slice(&[((pixel >> 16) & 0xFF) as u8, ((pixel >> 8) & 0xFF) as u8, (pixel & 0xFF) as u8]);
        }
        let png = png::encode_rgb((spec.w * 4) as u32, (spec.h * 4) as u32, &rgb);
        std::fs::write(out_tmp.join(&spec.file), &png).map_err(|error| error.to_string())?;
        if timing {
            eprintln!("mapbuild {}: {} bytes in {:.1}s", spec.file, png.len(), mark.elapsed().as_secs_f64());
        }
        files.push((spec.file.clone(), png.len() as u64));
    }
    if files.is_empty() {
        return Err("pack renders no painted tiles".to_string());
    }
    let rendered: Vec<&ImageSpec> =
        specs.iter().filter(|spec| files.iter().any(|(file, _)| file == &spec.file)).collect();
    let images = rendered
        .iter()
        .map(|spec| {
            object(vec![
                ("file", str_value(&spec.file)),
                ("level", Value::Int(spec.level as i64)),
                ("x0", Value::Int(spec.x0 as i64)),
                ("z0", Value::Int(spec.z0 as i64)),
                ("w", Value::Int(spec.w as i64)),
                ("h", Value::Int(spec.h as i64)),
            ])
        })
        .collect();
    let manifest = object(vec![
        ("hash", str_value(hash)),
        ("tile", Value::Int(4)),
        ("window", Value::Int(WINDOW_TILES as i64)),
        ("images", Value::Arr(images)),
    ]);
    let body = json::stringify(&manifest).into_bytes();
    std::fs::write(out_tmp.join("manifest.json"), &body).map_err(|error| error.to_string())?;
    files.push(("manifest.json".to_string(), body.len() as u64));
    Ok(files)
}

pub struct Report {
    pub hash: String,
    pub dir: String,
    pub skipped: bool,
    pub files: Vec<(String, u64)>,
    pub total_bytes: u64,
    pub elapsed_secs: f64,
}

impl fmt::Display for Report {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.skipped {
            write!(out, "static map {} up to date (SKIP, {:.1}s)", self.dir, self.elapsed_secs)
        } else {
            write!(
                out,
                "static map {} built: {} files, {} bytes in {:.1}s",
                self.dir,
                self.files.len(),
                self.total_bytes,
                self.elapsed_secs
            )
        }
    }
}

/// Build or skip the static spectator map. SKIP touches nothing (no
/// re-parse, no rewrite); a rebuild renders into `map-<hash>.tmp`, then
/// one rename makes it live — the served folder is never half-written.
pub fn ensure_static_map(static_dir: &Path) -> Result<Report, String> {
    let started = Instant::now();
    let zip = std::fs::read(pack_zip()).map_err(|error| format!("read {}: {error}", pack_zip().display()))?;
    let hash = fnv_hex(&zip);
    let dir = dir_name(&hash);
    if !needs_rebuild(static_dir, &hash) {
        purge_old(static_dir, &hash);
        let manifest = std::fs::read_to_string(static_dir.join(&dir).join("manifest.json")).unwrap_or_default();
        let files = json::parse(&manifest)
            .ok()
            .and_then(|parsed| parsed.get("images").as_arr().map(|items| items.to_vec()))
            .unwrap_or_default()
            .iter()
            .map(|image| {
                let file = image.get("file").as_str().unwrap_or("").to_string();
                let size = std::fs::metadata(static_dir.join(&dir).join(&file)).map(|meta| meta.len()).unwrap_or(0);
                (file, size)
            })
            .collect::<Vec<_>>();
        let total = files.iter().map(|(_, size)| size).sum();
        return Ok(Report {
            hash,
            dir,
            skipped: true,
            files,
            total_bytes: total,
            elapsed_secs: started.elapsed().as_secs_f64(),
        });
    }
    let maps = Minimap::new(mapcache::pack_dir());
    let tmp = static_dir.join(format!("{dir}.tmp"));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|error| error.to_string())?;
    }
    let files = build_all(&maps, &tmp, &hash)?;
    let total = files.iter().map(|(_, size)| size).sum();
    let target = static_dir.join(&dir);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
    }
    std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
    std::fs::write(static_dir.join(format!("{dir}.stamp")), format!("{hash}\n")).map_err(|error| error.to_string())?;
    purge_old(static_dir, &hash);
    let pointer = json::stringify(&object(vec![("dir", str_value(&dir)), ("hash", str_value(&hash))]));
    let pointer_tmp = static_dir.join("map-current.json.tmp");
    std::fs::write(&pointer_tmp, pointer.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&pointer_tmp, static_dir.join("map-current.json")).map_err(|error| error.to_string())?;
    Ok(Report { hash, dir, skipped: false, files, total_bytes: total, elapsed_secs: started.elapsed().as_secs_f64() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn squares(band: (i32, i32), xlo: i32, xhi: i32) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for square_z in band.0..=band.1 {
            for square_x in xlo..=xhi {
                out.push((square_x, square_z));
            }
        }
        out
    }

    #[test]
    fn plan_counts_sixteen_files() {
        let mut all = Vec::new();
        all.extend(squares((20, 20), 35, 35));
        all.extend(squares((44, 62), 32, 56));
        all.extend(squares((70, 77), 29, 47));
        all.extend(squares((144, 158), 36, 54));
        all.extend(squares((160, 161), 46, 47));
        let specs = plan_images(&all);
        assert_eq!(specs.len(), 16, "5 bands x non-empty levels");
        let mut files: Vec<&str> = specs.iter().map(|spec| spec.file.as_str()).collect();
        files.sort();
        assert!(files.contains(&"i20-l0.png") && !files.contains(&"i20-l1.png"));
        assert!(files.contains(&"c44-l3.png") && files.contains(&"c160-l2.png") && !files.contains(&"c160-l3.png"));
    }

    #[test]
    fn plan_skips_missing_band() {
        let all = squares((44, 62), 32, 56);
        let specs = plan_images(&all);
        assert_eq!(specs.len(), 4, "continent only");
        assert!(specs.iter().all(|spec| spec.file.starts_with("c44-")));
    }

    #[test]
    fn plan_margin_covers_edge_window() {
        let all = squares((44, 62), 32, 56);
        let specs = plan_images(&all);
        let spec = specs.iter().find(|spec| spec.level == 0).expect("continent L0");
        assert_eq!((spec.x0, spec.z0), (32 * 64 - 16, 44 * 64 - 16));
        // Painted-edge player still inside the file with a full window,
        // and one tile of ocean past the squares too (the margin's point).
        assert!(source_rect(spec, 32 * 64, 44 * 64).is_some());
        assert!(source_rect(spec, 32 * 64 - 1, 44 * 64 - 1).is_some());
        assert!(source_rect(spec, 32 * 64 - 17, 44 * 64 - 17).is_none());
    }

    #[test]
    fn source_rect_matches_renderer_rows() {
        // File rows run north-up: window top tile oz+32 lands at file row 0.
        let spec = ImageSpec { file: "t.png".to_string(), level: 0, x0: 100, z0: 200, w: 100, h: 80 };
        assert_eq!(source_rect(&spec, 100, 200), Some((0, 188)));
        assert_eq!(source_rect(&spec, 116, 216), Some((64, 124)));
        assert_eq!(source_rect(&spec, 99, 200), None);
        assert_eq!(source_rect(&spec, 167, 200), Some((268, 188)));
        assert_eq!(source_rect(&spec, 168, 200), None);
        assert_eq!(source_rect(&spec, 100, 247), Some((0, 0)));
        assert_eq!(source_rect(&spec, 100, 248), None);
    }

    #[test]
    fn hash_changes_with_pack_bytes() {
        assert_ne!(fnv_hex(b"a"), fnv_hex(b"b"));
        assert_eq!(fnv_hex(b"pack"), fnv_hex(b"pack"));
        assert_eq!(fnv_hex(b"pack").len(), 16);
    }

    #[test]
    fn skip_and_invalidation_without_pack() {
        let root = std::env::temp_dir().join(format!("mapbuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join("map-0123456789abcdef");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"images":[{"file":"c44-l0.png"}]}"#).unwrap();
        std::fs::write(dir.join("c44-l0.png"), b"fake").unwrap();
        std::fs::write(root.join("map-0123456789abcdef.stamp"), "0123456789abcdef\n").unwrap();
        assert!(!needs_rebuild(&root, "0123456789abcdef"), "stamp + files present skips");
        std::fs::remove_file(dir.join("c44-l0.png")).unwrap();
        assert!(needs_rebuild(&root, "0123456789abcdef"), "missing file rebuilds");
        std::fs::write(dir.join("c44-l0.png"), b"fake").unwrap();
        std::fs::write(root.join("map-0123456789abcdef.stamp"), "other\n").unwrap();
        assert!(needs_rebuild(&root, "0123456789abcdef"), "wrong stamp rebuilds");
        // Purge keeps current plus one previous complete pair.
        for other in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"] {
            std::fs::create_dir_all(root.join(format!("map-{other}"))).unwrap();
            std::fs::write(root.join(format!("map-{other}.stamp")), format!("{other}\n")).unwrap();
        }
        purge_old(&root, "0123456789abcdef");
        assert!(root.join("map-0123456789abcdef").is_dir());
        let survivors = ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"].iter().filter(|h| root.join(format!("map-{h}")).exists()).count();
        assert_eq!(survivors, 1, "only the newest previous pair survives");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_crops_match_live_frames() {
        // i20 is one square (fast): file pixels cropped by source_rect
        // must equal the direct frames /minimap.png used to serve.
        let maps = Minimap::new(mapcache::pack_dir());
        let spec = plan_images(&maps.map_squares())
            .into_iter()
            .find(|spec| spec.file == "i20-l0.png")
            .expect("ilot file planned");
        let file = maps.render_static(spec.level, spec.x0, spec.z0, spec.w, spec.h);
        assert!(file.iter().any(|pixel| *pixel != 0), "ilot paints something");
        let stride = spec.w * 4;
        for (ox, oz) in [(spec.x0 + 16, spec.z0 + 16), (2240, 1296), (spec.x0, spec.z0 + 30)] {
            let (sx, sy) = source_rect(&spec, ox, oz).expect("window covered");
            let (sx, sy) = (sx as usize, sy as usize);
            let mut crop = vec![0u32; 132 * 132];
            for row in 0..132 {
                crop[row * 132..(row + 1) * 132]
                    .copy_from_slice(&file[(sy + row) * stride + sx..(sy + row) * stride + sx + 132]);
            }
            assert_eq!(crop, maps.render_static(spec.level, ox, oz, 33, 33), "window {ox},{oz}");
        }
    }

    #[test]
    fn rect_render_matches_square_render() {
        // Non-square windows share the same code path as square ones: the
        // overlap of a 40x20 and a 40x40 render over the same tiles must
        // agree pixel for pixel (catches width/height stride mixups).
        let maps = Minimap::new(mapcache::pack_dir());
        let wide = maps.render_static(0, 3200, 3200, 40, 20);
        let tall = maps.render_static(0, 3200, 3180, 40, 40);
        // Same northern edge (tile 3219): the whole 40x20 window is the
        // top half of the 40x40 one.
        for row in 0..20 * 4 {
            let wide_row = &wide[row * 40 * 4..(row + 1) * 40 * 4];
            let tall_row = &tall[row * 40 * 4..(row + 1) * 40 * 4];
            assert_eq!(wide_row, tall_row, "overlap row {row}");
        }
    }

    /// Full 16-image build against the real pack: weight and layout proof.
    /// Slow (minutes in debug), so ignored in the default run.
    #[test]
    #[ignore]
    fn full_build_weights() {
        let maps = Minimap::new(mapcache::pack_dir());
        let hash = fnv_hex(maps.pack_bytes());
        let out = std::env::temp_dir().join(format!("mapbuild-full-{hash}"));
        let _ = std::fs::remove_dir_all(&out);
        let files = build_all(&maps, &out, &hash).expect("full build");
        let total: u64 = files.iter().map(|(_, size)| size).sum();
        let mut names: Vec<String> = files.iter().map(|(file, size)| format!("{size:>10}  {file}")).collect();
        names.sort();
        eprintln!("static map files:\n{}", names.join("\n"));
        eprintln!("total {total}");
        let pngs = files.iter().filter(|(file, _)| file.ends_with(".png")).count();
        assert!((15..=16).contains(&pngs), "one PNG per non-empty cluster level, got {pngs}");
        assert!(files.iter().any(|(file, _)| file == "manifest.json"));
        assert!(total <= 5_000_000, "over the 5 MB ceiling: {total}");
        let _ = std::fs::remove_dir_all(&out);
    }
}
