//! mod-helper: map tools for a HOI4 total conversion centred on Tyrol and
//! Trentino. The first tool is a layer viewer for rasters and OSM boundaries.

mod app;
mod bundle;
mod content;
mod editor;
mod geo;
mod modbuild;
mod layers;
mod mapproj;
mod models;
mod newproject;
mod osm;
mod project;
mod raster;
mod tiles;
mod util;
mod vector;
mod view;
mod welcome;
mod zip;

use std::path::PathBuf;

use eframe::egui;

const USAGE: &str = "\
usage:
  mod-helper                            the welcome screen: recent projects, new projects, imports
  mod-helper PROJECT.hoi4proj           open a project (an old PROJECT.project.json is converted first)
  mod-helper --editor PROJECT           open it with the map editor
  mod-helper --content PROJECT          open it with the nation content screen (focus trees, events...)
  mod-helper --info FILE...             print size, format and georeference of rasters
  mod-helper --extract-osm FILE.pbf...  extract (and cache) admin boundaries, rivers and lakes from OSM extracts
  mod-helper --export-geojson IN OUT.geojson [LON0,LAT0,LON1,LAT1]
                                        write boundary and water lines and area labels of an OSM extract
                                        (.osm.pbf or a cache .bin) as GeoJSON, optionally clipped
                                        to ways and labels touching a lon/lat box
  mod-helper --build-mod PROJECT|MAP_DIR [--game DIR] [--mods DIR]
                                        build the HOI4 mod of a project (or of a map folder) into the
                                        game's mod folder, then check it
  mod-helper --import-map OUT.hoi4proj [--mod MOD_DIR] [--game DIR] [--name NAME]
                                        a new project with the map of the base game (or of a mod
                                        over it), on a plain raster
  mod-helper --convert OLD.project.json NEW.hoi4proj
                                        make a project file of a project of the old format (its
                                        settings and map folder; those are only read)
  mod-helper --import-model MODEL PROJECT.hoi4proj [--name NAME] [--scale S | --height PX] [--up z]
             [--snow] [--bumps] [--folder F] [--package P]
                                        a new 3D model in the project's content: MODEL is a .glb,
                                        .gltf, .obj or a .mesh of Paradox's Blender exporter; its
                                        textures are made the game's DDS (see the Objects level)
  mod-helper --validate-mod MOD_DIR [GAME_DIR]
                                        check a built mod against the game's rules";

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        "info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,eframe=warn,egui_wgpu=warn,zbus=warn,tracing=warn,sctk_adwaita=error,arboard=error",
    ))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("--info") => cli_info(&args[1..]),
        Some("--extract-osm") => cli_extract(&args[1..]),
        Some("--export-geojson") => cli_export_geojson(&args[1..]),
        Some("--build-mod") => cli_build_mod(&args[1..]),
        Some("--validate-mod") => cli_validate_mod(&args[1..]),
        Some("--import-map") => cli_import_map(&args[1..]),
        Some("--convert") => cli_convert(&args[1..]),
        Some("--import-model") => cli_import_model(&args[1..]),
        Some("--editor") => run_gui(args.get(1).map(PathBuf::from), app::Start::Editor),
        Some("--content") => run_gui(args.get(1).map(PathBuf::from), app::Start::Content),
        Some(a) if a.starts_with("--") => anyhow::bail!("unknown option {a}\n{USAGE}"),
        other => run_gui(other.map(PathBuf::from), app::Start::Project),
    }
}

fn run_gui(project: Option<PathBuf>, start: app::Start) -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mod-helper · map viewer")
            .with_app_id("mod-helper")
            .with_inner_size([1600.0, 1000.0]),
        ..Default::default()
    };
    eframe::run_native("mod-helper", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, project, start)))))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn cli_info(files: &[String]) -> anyhow::Result<()> {
    let progress = util::Progress::default();
    for f in files {
        let t0 = std::time::Instant::now();
        match raster::load(f.as_ref(), &progress) {
            Ok((r, g)) => {
                println!("{f}");
                println!("  {} x {} px, {}, {} in memory", r.width, r.height, r.format, util::fmt_bytes(r.byte_size()));
                let (cx, cy) = (r.width / 2, r.height / 2);
                if let Some(v) = r.value_at(cx, cy) {
                    println!("  centre pixel ({cx}, {cy}): {v}");
                }
                let (lx, ly) = (r.width - 1, r.height - 1);
                if let Some(v) = r.value_at(lx, ly) {
                    println!("  last pixel ({lx}, {ly}): {v}");
                }
                if let Some((lo, hi)) = r.data_range {
                    println!("  value range {lo} .. {hi}{}", r.nodata.map(|n| format!(" (nodata {n})")).unwrap_or_default());
                }
                match &g.transform {
                    Some((gt, src)) => println!("  geotransform {:?} from {src}", gt.0),
                    None => println!("  no geotransform"),
                }
                match &g.crs {
                    Some((crs, src)) => println!("  {} from {src}", crs.name()),
                    None => println!("  CRS unknown"),
                }
                println!("  loaded in {:.2}s", t0.elapsed().as_secs_f32());
            }
            Err(e) => println!("{f}: error: {e:#}"),
        }
    }
    Ok(())
}

fn cli_build_mod(args: &[String]) -> anyhow::Result<()> {
    let (mut game, mut mods, mut target) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--game" => game = Some(PathBuf::from(it.next().ok_or_else(|| anyhow::anyhow!("--game needs a folder"))?)),
            "--mods" => mods = Some(PathBuf::from(it.next().ok_or_else(|| anyhow::anyhow!("--mods needs a folder"))?)),
            x if x.starts_with("--") => anyhow::bail!("unknown option {x}\n{USAGE}"),
            x => target = Some(PathBuf::from(x)),
        }
    }
    let target = target.ok_or_else(|| anyhow::anyhow!("--build-mod needs a project\n{USAGE}"))?;
    let mut dirs = modbuild::Dirs::default();
    let map_dir = if target.is_dir() {
        target
    } else {
        // the project's own settings: its game, mods folder and real-world data
        let (p, pf) = bundle::Project::open(&target)?;
        let ed = pf.editor.unwrap_or_default();
        dirs.game = ed.game_dir.clone().unwrap_or(dirs.game);
        dirs.mods = ed.mods_dir.clone().unwrap_or(dirs.mods);
        dirs.sources = modbuild::bitmaps::Sources { dem: ed.dem, cover: ed.landcover };
        p.map_dir()
    };
    dirs.game = game.unwrap_or(dirs.game);
    dirs.mods = mods.unwrap_or(dirs.mods);
    let log = |l: &str| println!("{l}");
    let mod_dir = modbuild::build(&map_dir, &dirs, &log)?;
    println!();
    let errors = modbuild::validate::validate(&mod_dir, &dirs.game, &log)?;
    if !errors.is_empty() {
        anyhow::bail!("{} checks failed", errors.len());
    }
    Ok(())
}

fn cli_import_map(args: &[String]) -> anyhow::Result<()> {
    let (mut out, mut game, mut mod_dir, mut name) = (None, modbuild::default_game_dir(), None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |what: &str| it.next().cloned().ok_or_else(|| anyhow::anyhow!("{what} needs a value"));
        match a.as_str() {
            "--game" => game = val("--game")?.into(),
            "--mod" => mod_dir = Some(PathBuf::from(val("--mod")?)),
            "--name" => name = Some(val("--name")?),
            x if x.starts_with("--") => anyhow::bail!("unknown option {x}\n{USAGE}"),
            x => out = Some(PathBuf::from(x)),
        }
    }
    let out = out.ok_or_else(|| anyhow::anyhow!("--import-map needs OUT.{}\n{USAGE}", bundle::EXT))?;
    let src = newproject::hoi4::Source { game: game.clone(), mod_dir };
    let size = project::bmp_size(&src.mod_dir.as_ref().map(|m| m.join("map/provinces.bmp")).filter(|p| p.is_file()).unwrap_or_else(|| src.game.join("map/provinces.bmp")));
    let pixel_m = size.map_or(7000.0, |(w, _)| 40_075_000.0 / w as f64);
    let name = name.unwrap_or_else(|| if src.mod_dir.is_some() { "Imported mod".into() } else { "Imported base game".into() });
    let t0 = std::time::Instant::now();
    let game_dir = Some(game).filter(|g| *g != modbuild::default_game_dir());
    let req = newproject::Request { name, file: out, start: newproject::Start::Import(src), proj: mapproj::Projection::Plain { pixel_m }, game_dir };
    let (_, c) = newproject::create(&req, &util::Progress::default())?;
    for l in c.report {
        println!("{l}");
    }
    println!("wrote {} in {:.1} s", c.project.display(), t0.elapsed().as_secs_f32());
    Ok(())
}

fn cli_convert(args: &[String]) -> anyhow::Result<()> {
    let [old, new] = args else { anyhow::bail!("--convert needs OLD.project.json and NEW.{}\n{USAGE}", bundle::EXT) };
    let (p, report) = bundle::convert(old.as_ref(), new.as_ref())?;
    for l in report {
        println!("{l}");
    }
    println!("wrote {}", p.path.display());
    Ok(())
}

fn cli_import_model(args: &[String]) -> anyhow::Result<()> {
    use models::import;
    let mut files: Vec<PathBuf> = Vec::new();
    let (mut name, mut scale, mut height, mut up_z, mut snow, mut bumps, mut folder, mut package) = (None, None, None, false, false, false, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |what: &str| it.next().cloned().ok_or_else(|| anyhow::anyhow!("{what} needs a value"));
        match a.as_str() {
            "--name" => name = Some(val("--name")?),
            "--scale" => scale = Some(val("--scale")?.parse::<f32>()?),
            "--height" => height = Some(val("--height")?.parse::<f32>()?),
            "--up" => up_z = val("--up")?.eq_ignore_ascii_case("z"),
            "--snow" => snow = true,
            "--bumps" => bumps = true,
            "--folder" => folder = Some(val("--folder")?),
            "--package" => package = Some(val("--package")?),
            x if x.starts_with("--") => anyhow::bail!("unknown option {x}\n{USAGE}"),
            x => files.push(x.into()),
        }
    }
    let [model, proj] = &files[..] else { anyhow::bail!("--import-model needs MODEL and PROJECT.{}\n{USAGE}", bundle::EXT) };
    let src = import::read(model)?;
    for n in &src.notes {
        println!("note: {n}");
    }
    let mut o = import::Options::for_source(&src);
    if let Some(n) = name {
        o.name = n;
    }
    if let Some(f) = folder {
        o.folder = f;
    }
    o.up = if up_z { import::Up::Z } else { import::Up::Y };
    o.snow = snow;
    if bumps {
        for m in &mut o.mats {
            if m.normal == import::NormalFrom::Flat {
                m.normal = import::NormalFrom::Bumps;
            }
        }
    }
    o.scale = scale.unwrap_or(1.0);
    if let Some(h) = height {
        // the height at scale 1, then the scale for the height asked
        o.scale = 1.0;
        let raw = import::make(&src, &o)?;
        let r = raw.mesh.max.y - raw.mesh.min.y;
        anyhow::ensure!(r > 0.0, "the model has no height");
        o.scale = h / r;
    }
    let made = import::make(&src, &o)?;
    for n in &made.notes {
        println!("note: {n}");
    }
    let (mut p, pf) = bundle::Project::open(proj)?;
    let pkg = package.unwrap_or_else(|| content::store::SHARED.to_string());
    let written = import::install(&made, &p.map_dir().join("content"), &pkg)?;
    for w in &written {
        println!("  content/{pkg}/{w}");
    }
    p.save(&pf, false)?;
    let (lo, hi) = made.bounds();
    let s = hi - lo;
    println!(
        "{}: {} triangles, {:.2} × {:.2} map px, {:.2} high; saved in {}",
        made.entity,
        made.mesh.triangles(),
        s.x,
        s.z,
        s.y,
        p.path.display()
    );
    Ok(())
}

fn cli_validate_mod(args: &[String]) -> anyhow::Result<()> {
    let [mod_dir, rest @ ..] = args else { anyhow::bail!("--validate-mod needs MOD_DIR\n{USAGE}") };
    let game = rest.first().map(PathBuf::from).unwrap_or_else(modbuild::default_game_dir);
    let errors = modbuild::validate::validate(mod_dir.as_ref(), &game, &|l| println!("{l}"))?;
    if !errors.is_empty() {
        anyhow::bail!("{} checks failed", errors.len());
    }
    Ok(())
}

fn cli_export_geojson(args: &[String]) -> anyhow::Result<()> {
    let [input, output, rest @ ..] = args else {
        anyhow::bail!("--export-geojson needs IN and OUT\n{USAGE}");
    };
    let bbox: Option<[f64; 4]> = match rest {
        [] => None,
        [b] => {
            let v: Vec<f64> = b.split(',').map(|s| s.trim().parse()).collect::<Result<_, _>>()?;
            let [a, b, c, d] = v[..] else { anyhow::bail!("bbox must be LON0,LAT0,LON1,LAT1") };
            Some([a, b, c, d])
        }
        _ => anyhow::bail!("too many arguments\n{USAGE}"),
    };
    let input = std::path::Path::new(input);
    let b = if input.extension().is_some_and(|e| e == "bin") {
        osm::load_cache_file(input)?
    } else {
        osm::load_boundaries(input, &util::Progress::default())?
    };
    let deg = |c: [i32; 2]| [c[0] as f64 * 1e-7, c[1] as f64 * 1e-7];
    let inside = |p: [f64; 2]| bbox.is_none_or(|b| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]);
    let mut features = Vec::new();
    for w in &b.ways {
        let pts: Vec<[f64; 2]> = w.coords.iter().map(|&c| deg(c)).collect();
        if pts.len() >= 2 && pts.iter().any(|&p| inside(p)) {
            features.push(serde_json::json!({
                "type": "Feature",
                "properties": { "kind": "way", "admin_level": w.level },
                "geometry": { "type": "LineString", "coordinates": pts },
            }));
        }
    }
    let n_ways = features.len();
    for w in &b.water {
        let pts: Vec<[f64; 2]> = w.coords.iter().map(|&c| deg(c)).collect();
        if pts.len() >= 2 && pts.iter().any(|&p| inside(p)) {
            features.push(serde_json::json!({
                "type": "Feature",
                "properties": { "kind": "water", "water": format!("{:?}", w.kind).to_lowercase() },
                "geometry": { "type": "LineString", "coordinates": pts },
            }));
        }
    }
    let (n_water, mut n_areas) = (features.len() - n_ways, 0);
    for a in &b.areas {
        if let Some(p) = a.label.map(deg).filter(|&p| inside(p)) {
            n_areas += 1;
            features.push(serde_json::json!({
                "type": "Feature",
                "properties": { "kind": "area", "name": a.name, "admin_level": a.level },
                "geometry": { "type": "Point", "coordinates": p },
            }));
        }
    }
    let fc = serde_json::json!({ "type": "FeatureCollection", "features": features });
    std::fs::write(output, serde_json::to_vec(&fc)?)?;
    println!("{output}: {n_ways} ways, {n_water} water lines, {n_areas} area labels");
    Ok(())
}

fn cli_extract(files: &[String]) -> anyhow::Result<()> {
    let progress = util::Progress::default();
    for f in files {
        let t0 = std::time::Instant::now();
        let b = osm::load_boundaries(f.as_ref(), &progress)?;
        let v = vector::VectorData::build(&b, geo::Crs::LAEA_EUROPE);
        println!("{f}: {} ways ({} points), {} areas, {:.1}s", v.ways.len(), v.n_points, v.areas.len(), t0.elapsed().as_secs_f32());
        for (lvl, n) in &v.level_counts {
            println!("  admin_level {lvl:>2}: {n:>6} ways");
        }
        for (w, n) in &v.water_counts {
            println!("  {:<16} {n:>6} lines", w.label());
        }
        println!("  bounds (EPSG:3035): {:?}", v.bounds.map(|x| x.round()));
        let mut names: Vec<&str> = b.areas.iter().filter(|a| a.level == 4).map(|a| a.name.as_str()).collect();
        names.sort();
        println!("  level-4 areas: {}", names.join(", "));
    }
    Ok(())
}
