# mod-helper

Map tools for the Tyrol / Trentino HOI4 total conversion:

1. a **layer viewer**: the rasters in `../maps/processed maps` and the
   administrative boundaries in `../maps/osm regions`, stacked as layers that
   can be toggled, reordered, moved, restyled and imported;
2. a **map editor** for the mod's map: provinces, states, nations, air and sea
   zones, adjacencies, state and nation properties, and inaccessible zones
   (see [Map editor](#map-editor));
3. the **mod builder**: makes the HOI4 mod from the edited map and checks it
   (see [Building the mod](#building-the-mod)). No Python needed.

## Running

```sh
cargo run --release                     # opens mod-helper.project.json (created on first run)
cargo run --release -- other.project.json
cargo run --release -- --editor         # start in the map editor
cargo run --release -- --info "../maps/processed maps/provinces.bmp"   # headless file info
cargo run --release -- --extract-osm "../maps/osm regions/austria-latest.osm.pbf"
cargo run --release -- --build-mod      # build the mod from the editor's map, then check it
cargo run --release -- --validate-mod ~/.local/share/Paradox\ Interactive/Hearts\ of\ Iron\ IV/mod/suedtirol_test
```

On first start, every BMP/TIFF/PNG in `../maps/processed maps` and every
`.osm.pbf` in `../maps/osm regions` is added. The height map and the province
map (45 % opacity) are visible; the other layers start hidden. The layer stack,
styles, offsets and view are saved to `mod-helper.project.json` on exit or
with Ctrl+S. Paths are stored relative to the project file.

The first time an `.osm.pbf` is loaded, its boundaries are extracted, which
takes a while for the 600–800 MB extracts. The result is cached in
`~/.cache/mod-helper/`, so later loads are instant. The cache is rebuilt when
the file changes.

## Controls

| Input | Action |
|---|---|
| Mouse wheel / pinch | zoom around cursor |
| Left drag (Pan tool), middle / right drag | pan |
| Left drag (Move tool, `M`) | move the selected layer |
| Arrow keys (Move tool) | nudge selected layer 1 px (Shift: 10 px) |
| `H` | show / hide selected layer |
| `F` | fit all rasters |
| `+` / `-` | zoom |
| Ctrl+I / Ctrl+S / Ctrl+O | import raster / save / open project |
| Drag & drop | import BMP / TIFF / PNG / JPEG / `.osm.pbf` |
| `F1` | help |

The side panel shows the layer list, the selected layer's properties, and an
inspector with the pixel coordinates and raw values (palette index, RGB,
height…) of every visible raster under the cursor.

Tests: `cargo test` runs the unit tests; `cargo test --release -- --include-ignored`
also checks the palette-TIFF decoder against the BMP export of the same data,
and runs the map editor on a copy of the real map (load / save round trip;
every edit operation followed by the checks, undo and redo). The copy goes to
`$MOD_HELPER_TEST_OUT` or the system temp folder.

## Map editor

Switch with **✏ Map editor** in the menu bar (or start with `--editor`). It
edits the map project in `../maps/processed maps/suedtirol`: `provinces.bmp`
plus `provinces.json` (names, types, states, nations, strategic regions,
adjacencies, overrides), as written by `make_provinces.py`. The viewer's
layers stay underneath ("Background layers" at the bottom of the panel).

**Levels** (keys `1`–`7`): what clicks select and what the actions work on.

| Level | Select / paint | Actions |
|---|---|---|
| Provinces | provinces; the paint brush redraws borders with the first selected province (only over the same type by default, optionally only within the same state / nation) | merge, divide into *n*, **generate by terrain**, dissolve, new (paint), make land / sea, split separate parts, new state / air zone / sea zone from the selection |
| States | states; paint moves provinces into the first selected state | merge, divide, dissolve, generate states, new nation, impassable on / off |
| Nations | nations; paint moves whole states | merge, divide, dissolve, share out land (each state to the nearest capital), generate states |
| Air zones / Sea zones | land / naval strategic regions | merge, divide, dissolve, generate |
| Adjacencies | straits (`sea`), `impassable` borders, links (empty type, optionally with an `adjacency_rule_name`); Link tool (`L`): click the two provinces | edit, recompute the crossing points, delete |
| Inaccessible | brush-select provinces | make impassable, remove from play, restore access |

**Varying detail.** "Generate by terrain" re-cuts the selected provinces with
the density model of `make_provinces.py`: small provinces in built-up areas
and on valley floors, larger ones on slopes, large ones above 2000 m (sizes in
"Generation parameters"). The *detail* factor scales a run, so a valley can be
cut finer and a mountain group coarser. The colour mode "province size vs.
terrain target" shows where provinces are smaller (blue) or bigger (red) than
the terrain asks for. Province bounding boxes always stay under 1/8 of the map
(HOI4's limit), pieces under the minimum size join a neighbour, and points
where four provinces meet are removed.

**Properties** of the first selected entity are edited in the panel and applied
as one undo step:

- Province: name, terrain (else derived from the land cover).
- State: name, owner, impassable, population (manpower), state category,
  buildings (infrastructure, factories, dockyards, air base, anti-air,
  refinery, silo, radar, rocket site, reactor), resources, victory points.
- Nation: name, tag, colour, language (name lists), ruling ideology,
  settlement rank, capital, whether it starts with a militia.
- Zone: name. Adjacency: name, type, sea province crossed, rule, ends.

Anything left on "auto" is decided by the mod builder.

**Impassable borders.** The Border tool (`I`, on every level) blocks the
border between two provinces, as vanilla does along some mountain ridges
(an `impassable` adjacency). Click a border (or next to one) to make it
impassable, click it again to open it. Drag across several borders to block
them all; a drag that starts on a blocked border opens them instead. Blocked
borders are drawn in red; units cannot cross them and railways go around.

**Inaccessible zones.** "Make impassable" turns the selection into impassable
states of their nation. This is what vanilla does with the Sahara and the
Himalayas: units cannot enter, and the owner's colour stays. "Remove from play"
turns it into HOI4 `lake` provinces: no owner, no state, no units, no supply.
They keep the look of land and stay in their air zone; vanilla has 8 lake
provinces in no state. This mode is **experimental** (not yet tried in the
game), so check `error.log` after a build. Both modes can merge the selection
into as few provinces as possible.

**Checks** run in the background after every edit: province size and box
limit, separate pieces, crossings, provinces without state or zone, zone
type, contiguity of states and zones, capitals, tags, straits. Click an issue
to go there.

**Saving and building.** Ctrl+S (or 💾) writes `provinces.bmp` and
`provinces.json`. The previous files are copied to `backups/<time>/` first; the
last 10 are kept. Ids are renumbered 1..n on save if anything was removed.
"Build mod" saves, builds the mod and checks it (see
[Building the mod](#building-the-mod)), and opens the "Build mod and play"
window with the log and the game launcher (also "▶ Play…").
`make_provinces.py` refuses to overwrite a map changed in the editor unless
run with `--force` (it then keeps a copy in `backups/`). Adjacency rules go in
`adjacency_rules.txt` next to `provinces.json`.

Edits that move pixels (divide, generate, merge, paint) carry impassable
borders along: where a walled-off province gives pixels to another, the new
contacts along the wall stay impassable, so dividing a province does not open
a gap. Walls between provinces that no longer touch are dropped.

**Mod.** The "Mod" section shows the mod's name, version and target folder.
"Mod details…" opens the metadata editor:

- Description: name (as the launcher lists it), version, author, description,
  tags (the launcher's list plus your own), thumbnail (PNG or JPEG, in the map
  folder or anywhere), game version (empty: the installed game's), Steam
  Workshop id (`remote_file_id`, once uploaded), dependencies (mods it loads
  after when they are enabled; load order, not a requirement).
- Build: mod folder name, bookmark country (a tag or part of a settlement
  name; empty: the biggest city), manpower of capitals and per km² of
  built-up land, relief of the normal map, seed of the random choices.
- Folders on this computer: the HOI4 install and where mods go. These are
  kept with the editor settings, not in the map.

"Apply" stores the details in `provinces.json` (`"mod"`) as one undo step.
"Build mod" applies pending changes first. A preview shows the
`descriptor.mod` that will be written. HOI4's descriptor has no author or
description field, so these go to a `README.txt` in the mod (and are there to
paste into the Workshop page).

"HOI4 map frame" keeps or removes vanilla's 3D metal border with the "Hearts
of Iron IV" logo. The frame sits at fixed positions made for the 5632×2048
vanilla map, so on a map of another shape it cuts through the map or ends
beside it. The setting is stored as `"map_frame"` in `provinces.json`.

| Input | Action |
|---|---|
| `1`–`7` | provinces / states / nations / air zones / sea zones / adjacencies / inaccessible |
| `S` / `B` / `L` / `I` / `P` | select / paint / link / border / pan tool |
| click, Shift / Ctrl+click | select; add or remove |
| drag (select tool) | select everything the brush touches |
| Alt+click | pick what is under the cursor |
| `[` / `]` | brush size |
| Delete | dissolve the selection |
| Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y | undo, redo (80 steps) |
| Esc | clear the selection |

## Building the mod

"Build mod" in the editor (or `--build-mod [MAP_DIR] [--game DIR] [--mods
DIR]`) makes the mod from the saved map in about 4 s. Inputs in the map
folder: `provinces.bmp` + `provinces.json`, `dem_section.tif` (heights) and
`landcover_smooth.bmp` (land cover classes). Palettes, texture sizes, name
lists and the game version come from the HOI4 install. The terrain colours,
tree type and city look are sampled from the vanilla Alps around Innsbruck.

It writes `<mods>/<folder>/` and `<mods>/<folder>.mod`:

- **map:** heightmap, terrain, normals, rivers, trees, cities, colour maps,
  `definition.csv`, buildings, unit / weather / supply positions, railways
  (a minimum spanning tree over neighbouring states, around impassable
  borders), adjacencies, strategic regions, and `ambient_object.txt` (the
  frame only if the option is on, ocean sounds over this map's sea zones, no
  Great Wall);
- **history:** states (category, manpower, buildings, victory points),
  countries and their militia;
- **countries:** one per nation, with tag, colour unlike its neighbours',
  procedural flag, and name lists by language;
- **other:** the bookmark, localisation, a tutorial pointed at this map (the
  vanilla one crashes on a new map), `descriptor.mod`, thumbnail, README.

Values set in the editor win over derived ones. The new folder is built next
to the old one and swapped in at the end, so a failed build keeps the last
good mod. A folder of that name is only replaced if mod-helper (or the old
`make_map_mod.py`) made it.

Then the mod is checked against the game's rules (also `--validate-mod
MOD_DIR [GAME_DIR]`): bitmap sizes and formats, provinces against
`definition.csv`, heightmap water, states and regions covering every province
once, positions inside their provinces, adjacencies, railways, building slots,
countries (tags, files, colours, flags, history, units, names, localisation),
bookmark, tutorial ids and the `.mod` path.

This replaces `make_map_mod.py` and `validate_mod.py`, which stay in
`maps/utils/edit_terrain` but are no longer used. Output matches theirs
except for the random choices (country colours, flags and building
positions use a different random generator) and ties in province centres.
`make_provinces.py`, which made the first province map from OSM and the DEM,
is still Python.

## Playing

The "Build mod and play" window (🔨 Build mod, or ▶ Play…) has a small game
launcher under the build log. It lists the mods the game can load: the
`.mod` files in its user folder (`~/.local/share/Paradox Interactive/Hearts
of Iron IV/mod`), with their version, the game version they are made for
(green if it fits the installed game, orange if not), and where they come
from: Steam Workshop, Paradox Mods, local, or local made by mod-helper. Mods
whose folder is gone are greyed out.

- Tick the mods to load. The ticks start as the Paradox launcher left them
  (`dlc_load.json`). A finished build ticks the mod it made.
- Search by name, filter by tags (mods with all ticked tags), or show only
  ticked mods. ↻ reads the mod folder again.
- **▶ Play** writes the ticked mods to `dlc_load.json` and starts the game as
  the Paradox launcher would (`launcher-settings.json`: `./run_hoi4
  -gdpr-compliant`). Load order: as ticked, except that a mod comes after the
  ticked mods its `dependencies` name. Play waits for a running build and is
  off while the game runs. Steam has to be running.

The ticks are written only on Play. The Paradox launcher keeps its own
playsets and writes `dlc_load.json` again when it starts the game.

## How layers are placed

All layers are drawn in one world CRS, **EPSG:3035 (LAEA Europe)**, the CRS
of the processed maps. A raster's georeference is read in GDAL's order:
`.aux.xml` GeoTransform/SRS, then internal GeoTIFF tags, then a world file
(`.wld`, `.tfw`, `.bpw`, …) and a `.prj` file. Rasters in another supported CRS
(EPSG:4326/4258, UTM 258xx/326xx/327xx, 3857) are reprojected on the fly by
warping the tile meshes.

A raster with no georeference (e.g. `sea.bmp`, or a plain HOI4 map) copies the
georeference of a loaded raster with the same pixel size. If there is none, it
is placed in the middle of the view. Either way the guess is saved as a manual
georeference, which you can edit in the "Georeference" section or copy from
another layer. The layer *offset* (Move tool / offset fields) is applied on
top, in metres.

## Architecture

| Module | Purpose |
|---|---|
| `raster.rs` | in-memory rasters in their **native** form (palette indices stay indices, heights stay heights); BMP (own codec: 1/4/8/24/32-bit, RLE8), GeoTIFF (parallel chunk decoding; own reader for 8-bit palette/gray/RGB(A) TIFFs, the `tiff` crate for 16-bit/float), PNG/JPEG; colour maps; tile rendering |
| `tiles.rs` | 512² tile pyramid, rendered on a worker pool, LRU-evicted above 1 GB of textures; style changes bump a generation so tiles re-render without flicker |
| `geo.rs` | CRSs and projections (LAEA, UTM, Web Mercator; checked against GDAL), geotransforms, sidecar parsing |
| `osm.rs` | three-pass `.osm.pbf` boundary extraction + cache |
| `vector.rs` | projected boundaries with Douglas–Peucker LODs, line styles per admin level |
| `layers.rs` | layer config (persisted) and runtime state, pixel ↔ world placement |
| `view.rs` | camera, raster/vector drawing, scale bar |
| `app.rs` | UI (egui/eframe 0.37, wgpu) |
| `project.rs` | project files and default layer discovery |
| `editor/model.rs` | map data (id tables, label bitmap, per-province statistics and borders), load / save with backups |
| `editor/doc.rs` | open map with transactions and undo / redo (data snapshots + pixel change logs) |
| `editor/ops.rs` | all edit operations: paint, merge, divide, generate, dissolve, states, nations, zones, adjacencies, inaccessible zones |
| `editor/kmeans.rs`, `editor/density.rs` | weighted k-means with the box limit; the terrain density model |
| `editor/validate.rs` | HOI4 map checks, run on a snapshot in the background |
| `editor/render.rs` | the overlay as a tile source (fill per province by colour mode, graded borders, selection) |
| `editor/mod.rs`, `editor/build.rs` | editor UI; Mod details; Build mod on a worker thread |
| `editor/launcher.rs` | the game launcher: mod list, `dlc_load.json`, starting the game |
| `modbuild/mod.rs` | the mod build: bitmaps, positions, railways, states, countries, bookmark, localisation, tutorial |
| `modbuild/validate.rs` | checks of a built mod |
| `modbuild/meta.rs` | mod metadata (stored in `provinces.json`), `descriptor.mod`, thumbnail |
| `modbuild/img.rs` | BMP / DDS / TGA I/O, Pillow-equivalent resampling, distance transform |
| `modbuild/vanilla.rs`, `countries.rs`, `rail.rs` | samples from the game; tags, colours, flags; railway graph |

Rasters sit behind `Arc<RwLock<Raster>>` and tiles are keyed by
`(layer, level, x, y)`, so editing tools can change pixels and invalidate only
the affected tiles.

## Limitations

- The map editor edits one map project at a time and keeps it in memory
  (about 0.3 GB for the 4608 × 2816 map with undo).
- "Remove from play" (lake provinces) is untested in the game.
- Rasters are loaded fully into RAM in their native format. The 40000² CLC+
  map takes about 1.6 GB.
- Band-separate (planar) multi-band TIFFs and RLE4 BMPs are not supported.
