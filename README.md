# mod-helper

Map tools for the Tyrol / Trentino HOI4 total conversion:

1. a **layer viewer**: rasters (BMP, GeoTIFF, PNG, JPEG) and the
   administrative boundaries, rivers and lakes of OpenStreetMap extracts,
   stacked as layers that can be toggled, reordered, moved, restyled and
   imported;
2. a **map editor** for the mod's map: provinces, states, nations, air and sea
   zones, adjacencies, state and nation properties, inaccessible zones, and
   the 3D objects on the map, with an animated model preview (see
   [Map editor](#map-editor));
3. a **nation content** editor: focus trees (visual), events, decisions,
   ideas, characters and the rest of what nations have, with checks, imports
   from the base game and other mods, and exports (see
   [Nation content](#nation-content));
4. the **mod builder**: makes the HOI4 mod from the edited map and its
   content and checks it (see [Building the mod](#building-the-mod)). No
   Python needed.

All of it belongs to a **project**: one file, made blank or imported from
the base game or a mod, on a plain raster or a map projection (see
[Projects](#projects)).

## Running

```sh
cargo run --release                                   # the welcome screen: recent projects, new, import, open
cargo run --release -- "My map.hoi4proj"              # open a project
cargo run --release -- --editor "My map.hoi4proj"     # start in the map editor
cargo run --release -- --content "My map.hoi4proj"    # start in the nation content screen
cargo run --release -- --info dem.tif                 # headless file info
cargo run --release -- --extract-osm austria-latest.osm.pbf
cargo run --release -- --build-mod "My map.hoi4proj"  # build the project's mod, then check it
cargo run --release -- --import-map "Base game.hoi4proj" [--mod MOD_DIR] [--name NAME]
cargo run --release -- --convert mod-helper.project.json "My map.hoi4proj"
cargo run --release -- --import-model chapel.glb "My map.hoi4proj" [--name NAME] [--height PX] [--snow] [--bumps]
cargo run --release -- --validate-mod ~/.local/share/Paradox\ Interactive/Hearts\ of\ Iron\ IV/mod/suedtirol_test
```

Rasters and OSM extracts are added with File → Import (Ctrl+I) or by
dropping them on the window. The layer stack, styles, offsets and view are
saved in the project file with Ctrl+S and on exit. Their paths are stored
relative to the project file (absolute when they share no folder with it).

The first time an `.osm.pbf` is loaded, its boundaries, rivers and lakes are
extracted, which takes about 10 s for the 600–800 MB extracts. The result is
cached in `~/.cache/mod-helper/`, so later loads are instant. The cache is
rebuilt when the file changes (and once when the cache format changes, as it
did when water was added).

An OSM layer draws:

- the administrative boundaries, styled per admin level, with area names;
- under them, the water: river centre lines (`waterway=river`), the outlines
  of river areas (`natural=water` + `water=river`, `waterway=riverbank`), the
  outlines of lakes and reservoirs (`natural=water`, `landuse=reservoir`, as
  closed ways or multipolygons), and canals (hidden at first). Ponds, basins
  and lakes under 1 ha are left out.

The layer's Display section has a row per admin level and per kind of water
(show, colour); "Line width" scales all of them.

## Releases

Publishing a GitHub release (`gh release create v0.1.0 --generate-notes`)
runs `.github/workflows/release.yml`, which builds the app and attaches it
to the release; nothing else runs it:

- Linux: `mod-helper-<tag>-linux-x86_64.AppImage` (needs glibc 2.39 or
  newer: Ubuntu 24.04, Fedora 40, Arch …). `chmod +x` it, then run it.
- Windows: `mod-helper-<tag>-windows-x86_64.exe`. It is not signed, so
  SmartScreen asks first (More info → Run anyway); a console window opens
  with it (the command line options print there).
- macOS: `mod-helper-<tag>-macos.zip`, the app for Apple silicon and Intel
  (macOS 11 or newer). It is not notarized: the first time, macOS refuses to
  open it; System Settings → Privacy & Security → Open Anyway.

On Windows and macOS the game is looked for where Steam puts it and the mods
go to `Documents/Paradox Interactive/Hearts of Iron IV/mod`; other folders
are set in the map editor's Mod details, under "Folders on this computer".

## Projects

A project is **one file**, `<name>.hoi4proj`: a ZIP archive (any zip tool
opens it) of
- its settings, `project.json`: the layers, the view, the editor's and the
  content screen's settings;
- its map, `map/`: `provinces.bmp`, `provinces.json`, the georeference, the
  painted terrain and imported heightmap, `adjacency_rules.txt`, and the
  nations' content in `content/`.

What a project only uses stays where it is, and the project refers to it
(relative to the project file): the rasters and OSM extracts of the layers,
the elevation and land cover the map was made from (map editor → Mod
details → Real-world data), the game and the mods folder.

**Working folder.** While a project is open, its map is unpacked into
`~/.local/share/mod-helper/projects/<name>-<hash>/map`. The editor, the
content screen and the build work on that folder.
- Every save packs what changed into the project file: the map, the
  content, a new projection, Ctrl+S, quitting. It writes a new file next to
  the project, then renames it over the old one. Unchanged files are copied
  as they are, so a save of the 4608 × 2816 map adds about 0.1 s.
- The editor's and the content screen's backups stay in the working folder,
  not in the project.
- The folder is used again while the project file is the one it was
  unpacked from. A project file changed elsewhere (copied over, restored) is
  unpacked again; the backups stay.
- If the app ended before packing changes it had saved, they are packed the
  next time the project opens.

**The welcome screen** opens when no project is given, or with File →
Welcome screen. It lists the projects opened before, with the mod's name,
the map's size and its projection, and makes new ones:
- **New project…**: a name, the file, and the map. A **blank** map is one
  land province (in a state, a nation and an air zone) or one sea province
  (in a sea zone). Its size is a preset (from vanilla's 5632 × 2048) or any
  size the game takes: multiples of 256, at most 13.2 million pixels.
- **Import a mod or the base game…**: the map of the base game, of a mod in
  the game's mod folder, or of any mod folder (see
  [Importing a map](#importing-a-map)).

**Where the map lies.** A map lies on either
- a **plain raster**: a size per pixel and no place on the Earth, for maps
  not made from geographic data, like vanilla's; or
- a **map projection** (LAEA Europe, UTM, Web Mercator or
  longitude/latitude, with its centre and pixel size), which places it on the
  Earth so that true geographic files (GeoTIFFs, OSM extracts) line up with it.

**File → Map projection…** changes it later. The map's pixels stay as they
are; only where they lie on the Earth changes, and the pixel size that areas
and slopes use. The old georeference is kept with the backups. The layers
are shown in the map's projection, and OSM layers need one.

**Old projects** (`mod-helper.project.json`, with the map in a folder of its
own such as `maps/processed maps/suedtirol`) are converted when opened. You
choose where the project file goes.
- The map's own files go into the project file.
- The map folder's `dem_section.tif` and `landcover_smooth.bmp` become the
  project's elevation and land cover references.
- The old files are only read, and the map's backups stay in its folder.

`--convert OLD NEW` does the same from the command line.

### Importing a map

The importer reads the game's files the way the game does: the mod's own
files first, then the game's, following the mod's `replace_path`s. A mod
packed in a `.zip` has to be unpacked first. Imports take about a second,
and a report lists what came over and what did not.

**What comes over:**
- provinces: colour, type, terrain, and the name of their victory point;
- states: owner, provinces, manpower, category, buildings, resources,
  victory points, impassable, cores (`add_core_of`, kept when they are not
  just the owner's) and the occupier (`controller`, when it is not the
  owner), of countries that own a state;
- the countries that own them: name, colour, capital, ruling party;
- strategic regions, adjacencies and their rules;
- rivers (traced back into lines), railways, supply hubs, naval bases and 3D
  objects;
- `terrain.bmp` (as painted terrain) and `heightmap.bmp` (as the imported
  heightmap).

**Fixed on the way.** A few things in the game's files break the editor's
rules or the build's checks:
- Strategic regions with both land and sea are split in two, "(land)" and
  "(sea)".
- Straits get crossing points inside their provinces.
- Straits between provinces that touch, impassable borders between provinces
  that do not, and adjacencies listed twice are dropped.
- Points where four provinces meet are fixed (about 20 px for vanilla).
- A state with more factories than its category has slots gets a bigger
  category.
- Canals (Kiel, Panama, Suez) stay: a `sea` adjacency between two sea
  provinces through land. The checks know them.
- Land and sea that touch only diagonally (vanilla has 155 such points) are
  a warning in the editor's checks, not an error.

**Not imported:** claims, dated history, province buildings other than naval bases, the
weather of strategic regions (the build writes its own), and sea provinces
listed in states.

Countries keep the game's tags as `source_tag`; the build gives them new
ones. Without real-world data, the heights come from the imported heightmap
(30 m per step above the water plane), or the land is flat (200 m); land
cover is grass wherever no terrain is painted. The imported base game builds
and passes all of the build's checks.

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

Only the layers that are shown are in memory. A raster is decoded whole (a
20,000 × 12,000 px float GeoTIFF takes a gigabyte, an OSM extract some
50–70 MB), so a hidden layer is not read when the project opens: it is read
when ticked, and dropped again when unticked, its memory handed back. Once
read, its size and georeference are kept, so a hidden layer can still be
framed ("Zoom to", "Fit all") and lend its georeference to a raster of the
same size; "Zoom to" on one never read shows it and frames it once read.

Tests: `cargo test` runs the unit tests. `MOD_HELPER_TEST_MAPS=<the mod's
maps folder> cargo test --release -- --include-ignored` also runs the tests
on the real data:
- it checks the palette-TIFF decoder against the BMP export of the same
  data;
- it runs the map editor on a copy of the real map: a load / save round trip,
  and every edit operation followed by the checks, undo and redo;
- it imports and builds the base game.

The copies go to `$MOD_HELPER_TEST_OUT` or the system temp folder. Tests
never touch mod-helper's own data folder.

## Map editor

Switch with **✏ Map editor** in the menu bar (or start with `--editor`). It
edits the project's map: `provinces.bmp` plus `provinces.json` (names,
types, states, nations, strategic regions, adjacencies, overrides). The
viewer's layers stay underneath ("Background layers" at the bottom of the
panel).

**Levels** (keys `1`–`9`, `0`, `G`): what clicks select and what the actions work on.

| Level | Select / paint | Actions |
|---|---|---|
| Provinces | provinces; the paint brush redraws borders with the first selected province (only over the same type by default, optionally only within the same state / nation); the **lasso** (`O`) draws a new province, or more of the selected one; **cut** (`K`) cuts one along a line | merge, divide into *n*, **generate by terrain**, dissolve, new (paint), new from an image, make land / sea / **lake**, split separate parts, new state / air zone / sea zone from the selection |
| States | states; paint moves provinces into the first selected state; the **lasso** (`O`) makes a new state of the provinces mostly inside it, or adds them to the selected one; **cut** (`K`) divides one along a line | merge, divide, dissolve, generate states, new nation, impassable on / off |
| Nations | nations; paint moves whole states | merge, divide, dissolve, share out land (each state to the nearest capital), generate states |
| Air zones / Sea zones | land / naval strategic regions | merge, divide, dissolve, generate |
| Adjacencies | straits (`sea`), `impassable` borders, links (empty type, optionally with an `adjacency_rule_name`); Link tool (`L`): click the two provinces | edit, recompute the crossing points, delete, **fix broken straits** |
| Inaccessible | brush-select provinces | make impassable, remove from play, restore access |
| Rivers | rivers; Draw tool (`D`): click a new river from its source to its mouth | move, add and remove points, widths, rename, reverse, simplify, delete, **generate from the heights** |
| Supply | railways, supply hubs, ports; Railway (`R`), Hub (`H`) and Port (`N`) tools | levels, delete, automatic or laid railways and hubs, a port in each coastal state, **supply estimate** |
| Terrain | provinces; the paint brush paints `terrain.bmp` (Alt+click picks the terrain under the cursor) | paint the selection, clear the painting, **import / export `terrain.bmp` and `heightmap.bmp`** |
| Objects | the 3D objects on the map (units, trees, buildings, landmarks…); Place tool (`A`): click to put down the model picked in the browser | move, turn, tilt, scale, lift, name, animation, always visible, duplicate, delete; **model browser** and **animated 3D preview** |

**Varying detail.** "Generate by terrain" re-cuts the selected provinces
with a province size for every pixel, set in "Generation parameters":

- *Land by height and slope* is a list of rules. Each rule has a name, a
  province size and optional limits: height from / below (m) and slope
  from / below (degrees, mean over 9×9 px); a limit left at "any" does not
  restrict. The first rule from the top that fits a pixel sets its size, and
  *other land* covers what no rule fits. Each rule shows how much of the
  land it covers, so a rule that rules above it already take shows up at 0 %.
  The defaults are those of `make_provinces.py`: valley floors (below 1600 m
  and 12°) 1.5 km², mountains (from 2000 m) 30 km², other land 8 km².
  Settings saved before the rules existed become these rules with their
  sizes.
- *Built-up areas* get smaller provinces where they ask for smaller ones
  than their rule.

The *detail* factor scales a run, so a valley can be cut finer and a
mountain group coarser. The colour mode "province size vs. terrain target"
shows where provinces are smaller (blue) or bigger (red) than the terrain
asks for. Province bounding boxes always stay under 1/8 of the map (HOI4's
limit), pieces under the minimum size join a neighbour, and points where four
provinces meet are removed.

**Winding borders.** Generated and divided provinces are cut around centres
spread over the area; each pixel goes to its nearest centre, which on its
own draws straight borders (Voronoi cells). Instead each pixel looks for its
centre from a slightly moved position: a smooth random displacement with
bends at every scale from the cell size down, scaled to the local province
size. The borders then wind like natural ones. States, nations and zones
that are generated or divided are grouped the same way, so their outlines
wind too. *Winding borders* in "Generation parameters" sets how much: 0 gives
the straight borders, 1 (the default) natural bends, 2 or more strongly
winding ones. A cut is the same each time it is repeated with the same
settings.

**Province from image.** "From image…" (Provinces level) makes one new
province with the shape of a colour channel of an image: every map pixel
where the red, green, blue, alpha or brightness value is at least (or below)
a threshold. The image can be any BMP, TIFF, PNG or JPEG. It is laid on the
map by its georeference when it has one (any resolution, reprojected if
needed), pixel for pixel if it has the map's size, and stretched over the map
otherwise. The window says which. A preview shows the shape on the map, with
the pixel count and the provinces it covers (those entirely inside it go).
The new province is land, sea or removed land ("lake"): by default the kind
most of the shape lies on, and only over provinces of that kind, so a land
shape leaves the sea alone. A land or sea province gets the state and air /
sea zone of most of its area; one too large or in separate pieces shows up in
the checks. Removed land works like "Remove from play": it takes land and
removed provinces but never sea, every part keeps its air zone (one province
per zone), and with "merge into as few provinces as possible" (the same
setting as in the Inaccessible panel) each is cut to HOI4's size limits.
Specks of the shape under 8 px stay as they are, what it leaves of a province
under 8 px goes with it, and crossings along its edge are fixed. One undo
step.

**Properties** of the first selected entity are edited in the panel. A change
goes into the map once it is done: a text field when you leave it (Enter, Tab
or a click elsewhere), anything else when you let go of it. Each change is one
undo step and changes only the fields you edited, so what other edits did to
the entity meanwhile stays. A change still being typed is applied before
anything that could lose it (selecting something else, another level, an
action, undo, save, build) and counts as unsaved. The fields:

- Province: name, terrain (else derived from the land cover).
- State: name, owner, impassable, population (manpower), state category,
  buildings (infrastructure, factories, dockyards, air base, anti-air,
  refinery, silo, radar, rocket site, reactor), resources, victory points,
  cores and "occupied by":
  - **population** (`manpower` of the state file): "auto" shows what the
    build gives it (the manpower base of Mod details › Build, a quarter
    outside the nation's capital state, plus the manpower per km² of
    built-up land, to the hundred; "at least" the base until the built-up
    land is read). Untick it to set the number yourself; it starts from the
    auto value. Below it, about how many soldiers the nation recruits from
    it under Volunteer Only (1.5%, the game's default law) and Limited
    Conscription (2.5%): where divisions without `start_manpower_factor`
    get their men.
  - **buildings** go only as high as the game allows. The caps come from
    the game's `common/buildings` and `common/state_category` (vanilla's
    values when the game is not installed): infrastructure 0-5, factories
    and dockyards 0-20, air base 0-10, anti-air 0-5, refinery 0-3, radar
    0-6, rocket site 0-3, reactor 0-1. Islands lower the air base cap. Air
    wings are stationed at the air bases (200 planes per level; the Air page
    of the nation content). The
    range is shown beside each building. A value over the cap, read from a
    file, stays as it is and is marked "at most 5" in red. A dockyard in a
    state that does not touch the sea is marked "needs a coast". The
    category shows its building slots ("town (4 slots)", or "auto: town"
    when it follows the built-up area). The line under the buildings counts
    the slots the ticked factories, dockyards, refineries, silos, rocket
    sites and reactors take, in red when there are more than the category
    has.
  - **cores:** the owner's alone (the default, "the owner (automatic)"),
    other nations' as well or instead, or nobody's. A core is a nation's
    own land: full manpower and factories from it, and the right to claim
    it back. Cores of a merged or removed nation follow it or go.
  - **occupied by:** a nation that holds the state at the start without
    owning it (HOI4 `controller`). Colour mode *nations* stripes it in the
    occupier's colour. The base game only starts occupations between
    countries at war, so pair it with a war in the nations' history (or
    expect the state to go back to its owner). A capital that starts
    occupied is a warning.

  So a state cored by nobody but held by an occupier is owner X, cores
  "nobody", occupied by Y.
- Nation: name, tag, colour, language (name lists), ruling ideology,
  settlement rank, capital. (Its divisions are made on the Army page of
  its content: the build places none.)
- Zone: name. Adjacency: name, type, sea province crossed (an id of the
  map; marked in red when it is not a sea province touching both ends, or
  for a canal not a land province), rule, ends.

Anything left on "auto" is decided by the mod builder.

**Lasso.** The Lasso tool (`O`, Provinces and States levels) draws a loop
around an area. While the mouse button is held it draws freehand; once you
let go, each click adds a corner joined by a straight line, and holding the
button again goes on freehand from there. Click the start point (it fills
in when the cursor is on it), double-click, or press Enter to close the
loop; it is then applied as one undo step. Backspace takes back the last
stroke or corner, Esc drops the loop. A dashed line shows how the loop will
close; where it crosses or goes round itself, everything it encloses
counts. "The loop makes" in the panel chooses between a new province or
state and more of the selected one; Alt+click picks the one to draw onto.

- Provinces, *a new province*: the area becomes one province, made as
  "From image…" makes one: the type most of it lies on, the state and zone
  of most of it, the name of the province it takes most of. With "only
  over" (the Paint tool's setting) it takes only that type, and only the
  state or nation most of it lies in. What it leaves of a province under
  8 px goes with it, specks under 8 px stay, and crossings along its edge
  are fixed. A loop over removed land works like "Remove from play".
- Provinces, *more of the selected province*: the area goes to the selected
  province, as the paint brush would paint it (with "only over"). A
  province it leaves under 8 px goes entirely; crossings are fixed.
- States: the land provinces more than half inside the loop become a new
  state (named after the biggest, owned by the nation that holds most of
  them), or join the selected state. To give a state an exact border,
  draw its provinces first.

**Cut.** The Cut tool (`K`, Provinces and States levels) divides along a
line: hold the mouse button and draw it across, from border to border; it
cuts when you let go, as one undo step (Esc while drawing drops the line).
The line is the pixels it passes through, side by side, so nothing stays
joined across it. An end that stops a little short (up to 20 screen points)
of the border of what the line mostly runs through is carried on to it; an
end inside something else stays, so a line can start in a neighbour.

- Provinces: every province the line divides is cut. Its biggest piece
  keeps the province; each other piece becomes a new province like it (same
  type, state, zone and terrain). The line's own pixels go to the pieces
  beside them, bits under 8 px are not cut off, and crossings are fixed.
- States: every state the line divides is cut. Each province goes whole to
  the side most of it lies on; with "cut the provinces it crosses", those
  are cut first, so the new border follows the line. The biggest side keeps
  the state; each other side becomes a new state of the same nation, named
  like the divided ones ("Tirol II"). Victory points go with their
  provinces, and a manpower set by hand is shared out by size (as Divide
  does too now).

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

**Lakes.** "Make lake" (Provinces level, next to "Make land" and "Make
sea") turns the selection into lakes of water, like vanilla's: HOI4 `lake`
provinces (terrain `lakes`, no owner, no state, no units) under the water
plane. Give a province the lake's shape first (the paint brush, or "From
image…"), then make it a lake. Land and removed land keep their air zone. A
sea province joins the air zone of a neighbouring land. "Make land" turns a
lake back into land (it joins a neighbouring state), and the inaccessible
tools leave lakes alone. In the build a lake is water like the sea: shallow
at the shore and deeper inside in `heightmap.bmp`, ocean in `terrain.bmp`,
water in `rivers.bmp` (rivers end at it and start again after it), and the
game's water colours. The colour modes show lakes as water ("land / sea /
lake / impassable / removed" in teal, `terrain.bmp` and the heightmap as
water), and the terrain painting skips them. Removed land stays land in all
of this. A lake is stored as `"water": true` on the province in
`provinces.json`.

**Rivers.** Rivers are lines through map points, each point with a width
from 1 (narrowest) to 9; widths up to 4 count as small rivers in the game.
A river runs from its source to its mouth. Its pixels in `rivers.bmp` are
worked out from the lines, so the bitmap always follows the game's rules: one
pixel wide, a source marker at the start, a marker where a river flows into
another (or branches off one), no blocks of four or corner-only contacts.
A river ends at the first sea pixel. Over a lake (of water or removed land)
it is cut into pieces, each with its own source. Wider rivers are drawn first. A
narrower one that meets one flows into it and ends there, and one that
starts on one branches off it. Where it cannot join cleanly (alongside
another, or into a crowded spot), it ends just before. The panel shows the
`rivers.bmp` pixel count and lists rivers that go on past the sea or past
the river they flow into (the rest is left out) and rivers too short to
draw; click one to go there.

- Select tool: click a river to select it, drag its points, Shift+click on
  the river to add a point, Ctrl+click on a point to remove it, Del deletes
  it.
- Draw tool (`D`): click from the source down to the mouth. Double-click,
  right-click or Enter ends the river, Backspace takes back a point, Esc
  cancels. New points get the width of the slider.
- The selected river can be renamed, reversed (the other end becomes the
  source), simplified, and given widths that grow evenly from one value at
  the source to another at the mouth.
- *From the heights* follows the water down the elevation to the sea or
  the map's edge, filling hollows on the way. A river starts where the water
  of at least the set area gathers (*km² drained at least*: less makes more
  and longer rivers), each junction keeps the river with the larger
  catchment as the main one, and widths grow with the land drained. This
  runs in the background. The new rivers either replace the current ones or
  are added to them.
- Zoomed in far enough, *show the rivers.bmp pixels* draws the pixels as the
  build will write them: green is the source, red flows into another river,
  yellow branches off one, and darker blue is wider.

Rivers are shown on every level (the *rivers* checkbox at the top), and
they are stored in `provinces.json` (`"rivers"`).

**Supply.** This level edits what HOI4 builds its supply from, and shows an
estimate of the supply each province gets (colour mode *supply (estimate)*,
the default here).

- *Railways* are lines through stop provinces, each with a level (1–5).
  Each leg between two stops follows the shortest way over passable land,
  around impassable borders and impassable states; straits and water links
  count as 50 px (1.75 km) longer. The editor and the build route the same
  way. As long as none are laid, the build joins neighbouring states with
  level-1 railways (a minimum spanning tree between the main victory points
  of the states), and the map shows them. "Make editable" (or the first
  change) keeps those railways so you can change them. "Back to automatic"
  drops the laid railways again. Railway tool
  (`R`): click the stops; double-click, right-click or Enter ends the
  railway, Backspace takes back a stop, Esc cancels. The way is previewed
  while you draw. New railways get the level of the slider; the selected one
  can change its level. Where two railways share a link, the higher level
  counts.
- *Supply hubs* are automatic too (the main victory point of each state)
  until one is changed. Hub tool (`H`): click a province to add a hub, or to
  remove the one there. Hubs with no railway or river to their capital show
  red and are listed.
- *Ports* (naval bases, level 1–10) go in coastal provinces. Port tool
  (`N`): click a province on the sea to add one (with the slider's level),
  or to remove the one there. "One per coastal state" puts one in the most
  built-up coastal province of each state on the sea that has none.
  Fleets are stationed at them (the Navy page of the nation content).
- The *estimate* follows the game's supply rules (`NSupply` in
  `common/defines`). Supply starts at 5 in the capital, and at 2.8 in hubs
  joined to it by railway or river (+0.34 per level of the weakest railway on
  the way). Naval bases give 3.3 once one of them is joined to the capital.
  On each province it travels, supply loses a penalty that grows with the
  distance. The penalty is larger in rough terrain (forest, hills,
  mountains, marsh) and over rivers, and smaller with infrastructure and in
  cities. Supply from several sources adds up, and 1 or more fully supplies a
  division. It is worked out for each nation from its own capital, or for
  all the land as one country with the capital of a chosen nation. The
  hover panel shows a province's supply, terrain, infrastructure, railway
  level, hub, port and whether it lies on a river.
  Motorization, damage, weather, air superiority and fleets are left out,
  and terrain and infrastructure are taken as set in the editor, else as
  the build derives them. Treat it as a guide to where supply is short.

Railways, hubs and ports are stored as map points in `provinces.json`
(`"railways"`, `"supply_hubs"`, `"ports"`), so they stay in place when
provinces are re-cut or merged. The automatic railways and hubs are not
stored: a map without them follows its provinces and states.

**Terrain.** This level edits `terrain.bmp`, the ground textures and colours
of the 3D map (colour mode *terrain.bmp (as built)*, the default here). The
build makes it from the land cover and the slope of each pixel: built-up
land is urban, snow is snow, rock and moss are mountain, broadleaf and
conifer forest are the two forests; shrubs are alpine grass from 25° and
hills below; grass is alpine grass from 25°, hills from 10° and plains below;
cropland is hills from 10° and farmland below; anything else is plains. Trees
go where it has forest. It is only the look:
the terrain units fight in is the province's (Mod details → Terrain types).

- The Paint tool (`B`) paints the brush's terrain over it; "as built" takes
  the painting away again. The brush offers the build's nine terrains and,
  folded away, the rest of vanilla's (desert, jungle, marsh and other
  mountains). Alt+click picks the terrain under the cursor. The sea and
  lakes are always ocean. "Paint them" paints the selected provinces; "Clear all painting"
  goes back to the build's terrain everywhere. Each stroke is one undo step.
- *Export…* writes `terrain.bmp` or `heightmap.bmp` as the build would make
  them (with the painting or an imported heightmap), to edit in an image
  editor. *Import…* takes one back. It must be the map's size, and
  `terrain.bmp` must be an 8-bit bitmap with the game's palette, or 24-bit
  in its colours. Pixels of an imported `terrain.bmp` that differ from the
  build's terrain become painted; the others stay the build's, so they keep
  following the land cover and the coast. An imported heightmap (BMP, PNG or
  TIFF, grey) is used instead of the one from the elevation. The build
  fits it to the sea: land at or under the water plane (95) goes to 96, and
  sea and lakes at or above it take the build's depth. The shading (`world_normal.bmp`)
  adds what it changed to the measured heights. "Use the build's" drops it.
  Exporting and importing back changes nothing. Imports are one undo step.
- The colour mode *heightmap.bmp (as built)* shows the heightmap: grey on
  land, blue under the water plane. The hover panel shows the terrain or
  height under the cursor (painted or as built; imported or the build's, in
  metres).

The painting and the imported heightmap are saved next to `provinces.json`
as `terrain_paint.bmp` (255 where the build decides) and
`heightmap_import.bmp`, and go into the backups. A map without them gets the
build's.

**Objects.** This level places the game's 3D models on the map: what the
build writes to `map/ambient_object.txt`. Any entity or mesh of the game,
its DLCs and the mod (`<map>/content/*/gfx/`) can be placed: units, trees,
buildings, landmarks, ships, planes.

- **Models** is a browser of all of them (about 3300 in the base game and
  DLCs). It has a search over names and files, a filter by where they come
  from (the game, a DLC, the mod), "animated", a folder tree (by the
  folders of `gfx/models`), and a picture of each one. ↻ reads the models
  again after adding files to the mod's content.
- **Preview** shows the picked model in 3D with its textures, as the game
  would draw it. It is skinned to its skeleton, with the entities the game
  attaches (a soldier's rifle, a tank's crew). *plays* chooses an entity's
  state (`idle`, `move`, `attack`…) or any of the mesh's animations. Play,
  pause, speed and a time slider control it. Drag to turn it, use the wheel
  to zoom, and double-click to reset. *On the map* shows the map around
  the selected objects (or the last place clicked) in 3D, with the objects
  as the build will set them: height from the heightmap, water at the
  water plane. "⛶ Bigger…" moves the preview to a window of its own.
- **Place** (`A`) puts down the picked model where you click. Its scale,
  turn ("random" for a new one each time) and visibility come from *New
  objects*, and it plays what the preview plays.
- **Select** clicks objects (Shift / Ctrl add and toggle) or drags a box.
  Drag them to move them. Drag the yellow knob to turn them (Shift snaps
  to 15°). Alt+click picks an object's model in the browser.
  - `[` / `]` turn by 15° (Shift: 1°), arrows move by a pixel (Shift: 10).
  - Ctrl+D duplicates, Del deletes, Esc clears the selection.
  - The panel edits the selection: name, place, turn, tilt and roll,
    scale, lift above the ground, what it plays, always visible. "Use the
    browser's model" swaps the model.
- The map draws each object as its picture seen from above, turned and
  scaled like the object, and a dot when it is too small to see
  ("objects" in the display options shows them on every level).

**New models.** *Import a model…* (under Models) makes a new model for
the game out of a 3D file and its pictures. It does in one step what
Paradox's Blender exporter and the modding guides
([this one](https://steamcommunity.com/sharedfiles/filedetails/?id=3004523941))
do by hand: the `.mesh`, the DDS textures, the `pdxmesh` and the entity.

- **The 3D file** can be:
  - a glTF (`.glb`, or `.gltf` with its `.bin`) or an OBJ (with its
    `.mtl`), as Blender exports them (File → Export), so no plugin is
    needed;
  - a `.mesh` written by Paradox's exporter (io_pdx_mesh). Its bones,
    skin and locators are kept as they are.

  Its triangles, normals, texture coordinates and materials are read, with
  the transforms of a glTF's nodes. Blender's exporters write y up and
  right-handed; the game is y up and left-handed. So z is mirrored and the
  faces turned around, and the model looks as it did in Blender. *up* picks
  Z for an OBJ exported with Blender's own axes. Faces without normals are
  drawn flat.
- **Its pictures** can be DDS, PNG, JPEG or TGA. They are found where the
  file says, or by their name next to it or in a `textures` folder, and a
  GLB's own pictures are read from inside it. Each material gets three
  textures, written as the game's shader (`PdxMeshAdvanced` in
  `gfx/FX/pdxmesh.shader`) reads them:
  - **diffuse**: the colour picture, tinted by the material's colour (a
    glTF's base colour), or the plain colour when there is no picture;
  - **normal** (*bumps*): the game keeps x in green and y in alpha, and
    uses blue as a glow at night. The model's normal map is converted
    (Blender's green points up; tick *DirectX* for Substance's). A grey bump
    map, or *from the colour's brightness* (what the guide does with
    Paint.NET's emboss), becomes bumps of the chosen strength. *none* is
    flat;
  - **specular** (*surface*): the game keeps shine in green, metal in blue
    and gloss in alpha. It comes from a glTF's metal-roughness map, or the
    same all over from the sliders (a glTF's and an MTL's metal and
    roughness set them), or a picture already in that layout;
  - **glow** at night: a glTF's emissive map, an MTL's `map_Ke` / `Ke`, or
    any picture (bright is lit: windows, lamps).

  Each side is made a power of two (at most the size chosen, 1024 px at
  first). They are written uncompressed (B8G8R8A8) with every mipmap.
- **size**: map pixels per unit of the file, or the height in map pixels.
  On the game's map a tree is about 0.5 map px high and Big Ben about 6.
  This is the `pdxmesh`'s `scale`. *stands on the middle of its bottom*
  moves the model so its feet are where it stands. *snow* uses
  `PdxMeshAdvancedSnow`, which turns the model white in winter as the
  game's buildings and trees do.
- The **preview** shows the model as it will be written (the `.mesh` read
  back), with its colours. The game adds the bumps, shine, glow and snow.
- **Import** writes into a content package, `shared` by default:
  - `gfx/models/<folder>/<name>.mesh` and its textures
    (`<name>[_<material>]_diffuse.dds`, `_normal.dds`, `_specular.dds`);
  - `gfx/entities/<name>.gfx`, with the `pdxmesh` `<name>_mesh`, its scale
    and a `meshsettings` per material;
  - `gfx/entities/<name>.asset`, with the entity `<name>_entity`.

  The browser then reads the models again and picks the new one, ready
  for the Place tool. Importing a name again replaces that model, and
  the files it replaces go to `content/.backups/`. A name the game or a
  DLC already has is refused. A mesh with more than 65535 vertices is cut
  into several. `--import-model` does the same from the command line.

The objects are saved in `provinces.json` (`ambient_objects`): the model,
the map pixel, lift, rotation (degrees about x, y, z; y is the turn,
clockwise from north), scale, animation, visibility and name. In the build:

- objects of one model, scale, animation and visibility share a `type`
  block;
- a model played in another state than its default gets a clone of its
  entity with that `default_state`;
- a mesh without an entity, or a second scale or visibility of one, gets an
  entity of its own. These go in `gfx/entities/mod_helper_ambient.asset`;
- a DLC's model gets `dlc_allowed`, so it shows only to players with the
  DLC;
- a skinned model (its bones move its vertices: planes, ships, soldiers)
  set standing still plays its default state instead, else "idle", else its
  first animation. The game draws such models only animated; with
  `use_animation = no` their triangles fly across the screen. The panel
  warns when this happens, and the build log lists them;
- the game sets each object on the terrain and takes its y as a height
  above it, so the build writes the lift. On the sea and lakes it adds the
  depth up to the water plane, so the model stands on the water. Written
  with the ground's own height (as older mods' landmarks are), objects float
  that high: the first builds of this feature did;
- the rotation is turned into the game's: the editor turns clockwise like
  a compass (90° faces east), but the game's `rotation` y turns
  counter-clockwise, so the build writes the negative (a turn of 336° is
  `0 24 0`). This was taken from vanilla: naval bases in `buildings.txt`
  face their sea at 180° minus their rotation, and the Golden Gate Bridges
  of Kaiserreich and The Fire Rises only span the bay turned that way.

The check of the built mod looks for each object's entity in the mod and
the game, and checks that it is on the map.

**Checks** run in the background after every edit: province size and box
limit, separate pieces, crossings, provinces without state or zone, zone
type, contiguity of states and zones, capitals, tags, straits, rivers cut
short at the sea or another river (or too short to draw), railway stops off
passable land or without a way between them, hubs off passable land, ports
not on a coast. A state's buildings are checked against the game's caps,
including a building over its cap, a dockyard inland, two that exclude each
other (rocket site and mega gun), and a building or category the game does
not have (all errors). More buildings that take slots than the category has
is a warning; the automatic category counts once the terrain is loaded.
Click an issue to go there.

**Saving and building.** Ctrl+S (or 💾) writes `provinces.bmp` and
`provinces.json` (and the Terrain level's `terrain_paint.bmp` and
`heightmap_import.bmp` when there are any) into the project file. The
previous files are first copied to `backups/<time>/` in the working folder
(UTC time of the save); the last 10 are kept. Ids stay as they are when
something is removed (the gap stays), so what the nation content names by
number (a division's province, a capital state) keeps naming the same place,
and the id of something deleted is never given to something new
(`provinces.json` keeps `next_ids`); the build numbers them 1..n for the game and changes the content's numbers
along (see [Building the mod](#building-the-mod)). "Backups…" lists them with their age and province
count; "Load" puts one in the editor as one edit (Undo goes back). Nothing is
written until you save. The mod details and the map frame setting stay as they are.
Reload asks before it drops unsaved changes.
"Build mod" saves, builds the mod and checks it (see
[Building the mod](#building-the-mod)), and opens the "Build mod and play"
window with the log and the game launcher (also "▶ Play…").
Adjacency rules go in `adjacency_rules.txt` next to `provinces.json`.

Edits that move pixels (divide, generate, merge, paint) carry impassable
borders along: where a walled-off province gives pixels to another, the new
contacts along the wall stay impassable, so dividing a province does not open
a gap. Walls between provinces that no longer touch are dropped.

They carry straits along too. When a re-cut gives a strait's land or sea to
other provinces, each end moves to the province that now has the land
touching that sea nearest the old crossing point, and the sea to the one in
the middle of the crossing; default names follow. "Fix straits" (under
Checks, and in the Adjacencies list with the number of broken straits) does
the same for every strait whose land or sea no longer touches across the
water, for example from edits made before this was automatic. One undo step;
straits it cannot place are listed.

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
- Terrain types: the rules that give land provinces their HOI4 terrain
  (below).
- Folders on this computer: the HOI4 install and where mods go. These are
  kept with the editor settings, not in the map.

"Apply" stores the details in `provinces.json` (`"mod"`) as one undo step.
Closing the window, Save and "Build mod" apply pending changes too; while they
are invalid (no name, a bad folder name) the window stays open until they are
fixed or reverted. A preview shows the
`descriptor.mod` that will be written. HOI4's descriptor has no author or
description field, so these go to a `README.txt` in the mod (and are there to
paste into the Workshop page).

**Terrain types.** The build gives each land province a HOI4 terrain
(`definition.csv`; it sets supply, combat and movement) by rules checked from
the top. A rule is a terrain and the land it needs: one or more kinds of
land, each a land cover (built-up, forest, shrubs, grassland, cropland, rock
and moss, water, snow and ice, or any) within an optional height and slope
range (slope of the pixel), counted with a weight. The first rule whose land
makes up at least its share of the province sets the terrain; land no rule
takes gets the "other land" terrain. The defaults are what the build did
before the rules could be edited: urban from 20 % built-up land; mountain
where land of 25° or more plus half the land from 2200 m make up 55 %;
plains from 35 % land under 10°; forest from 55 % forest; else hills. A
terrain set on a province wins, and provinces at the map's edge are
mountain. Each rule shows how many provinces it takes and warns when it takes
none. The colour mode *terrain type (as built)* shows the result as you edit
the rules (before Apply). It has a legend with a "Terrain rules…" button, and
the hover panel shows the rule that decided and each rule's share against
its threshold. The supply estimate uses the same terrain. The rules are
stored with the mod details (`"mod"` → `"terrain"`).

"HOI4 map frame" keeps or removes vanilla's 3D metal border with the "Hearts
of Iron IV" logo. The frame sits at fixed positions made for the 5632×2048
vanilla map, so on a map of another shape it cuts through the map or ends
beside it. The setting is stored as `"map_frame"` in `provinces.json`.

| Input | Action |
|---|---|
| `1`–`9`, `0` | provinces / states / nations / air zones / sea zones / adjacencies / inaccessible / rivers / supply / terrain |
| `S` / `B` / `O` / `K` / `L` / `I` / `P` | select / paint / lasso / cut / link / border / pan tool |
| `D` / `R` / `H` / `N` | draw a river / railway / supply hub / port (naval base) tool (on the Rivers and Supply levels) |
| Enter, Backspace | end a new river or railway; take back its last point |
| click, Shift / Ctrl+click | select; add or remove |
| drag (select tool) | select everything the brush touches |
| Alt+click | pick what is under the cursor |
| `[` / `]` | brush size |
| Delete | dissolve the selection |
| Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y | undo, redo (80 steps) |
| Esc | clear the selection (while drawing a lasso loop or a cut: drop it) |
| lasso: hold / click; Enter or double-click; Backspace | draw freehand / add a corner; close the loop; take back its last part |

## Nation content

📜 **Nation content** (or `--content`) edits what the nations of the map
have in the game beyond the map: focus trees, events, decisions, ideas,
characters, country history, division templates and some 30 more kinds. It
does what the Python `hoi4-modmaker` does.

Content lives in the map folder, `<map>/content/<package>/`. There is one
package per nation (its tag) and `shared` for the whole mod's own. Each is
laid out like a mod (`common/`, `events/`, `history/`, `localisation/`,
`gfx/`, `interface/`). It is read into memory when the screen opens, edited
there with its own undo / redo, and written by Save (Ctrl+S, which also
saves the map). A file that is overwritten or deleted goes to
`content/.backups/<time>/` first (the last 10 backups are kept, model imports included). Packages are named by tag, so a nation
with content gets the tag the build gives it fixed in the map. The map then
has unsaved changes.

The nations are down the left. For the one picked:

- **Overview:** its tag, capital and states, how much content of each kind
  it has, new definitions, and its import, export and check.
- **Focus trees:** the tree as the game draws it: icons on the 96 × 130 grid,
  prerequisite lines (dashed where one of several is enough), red links
  between focuses that exclude each other, and the continuous focus box.
  - Modes: **S**elect (click, Shift/Ctrl-click, drag a box; drag focuses to
    move them on the grid), **L**ink (click a focus, then the one that needs
    it; Shift: one of several), **E**xclusive (click two focuses), **A**dd
    (click an empty cell), **P**an.
  - Keys: wheel zooms, the middle button pans, Delete removes the selection
    and what names it, Ctrl+D duplicates, arrows nudge (Shift: 5 cells),
    F fits.
  - Also: the inspector of the selected focus (every field, with a game-like
    preview), right-click menu, find, outline, minimap, DLC filter, hidden
    focuses, tidy (an automatic layout that leaves no overlaps), copy or
    shift a branch, new trees, and the tree's checks.
  - **Play** mode: click focuses to complete them and see what is available
    next, what still waits for its prerequisites, and what a mutually
    exclusive choice shut off.
  - Export: a PNG of the whole tree. Import: a tree from a draw.io diagram.
- **Kinds:** a list with filter and groups, then one definition as a form
  (fields with help, sprite pictures and a sprite picker, links to what the
  ids name, a preview of events, ideas and decisions) or as code
  (highlighted, with errors marked).
  New, duplicate, rename, delete, copy to another nation.
  - What a field does (every field of every kind): hover its name, or
    click it or its **?** to keep the explanation open under it: what the
    field does in the game and how it
    is used (`create_faction` founds a faction led by the country and takes
    its name, a localisation key; `recruit_character` hires a character;
    `capital` and `oob` come after what the build writes and win, ...),
    what it takes, the field as written in a file, and for a country's
    history (run as effects when the game starts) the game's own
    documentation of the effect.
  - Fields of a country, a state, a character, an OOB file or a technology
    sharing group have a list to pick from beside them and show what the
    value names (the nation, the state, a missing localisation text, a
    file that is not there).
  - Number fields take only what the game allows, for example:
    - stability and war support 0 to 1;
    - a template's priority 0 to 2;
    - an idea's `removal_cost` -1 or 0 and more;
    - whole numbers of days, slots and convoys;
    - a subject's freedom 0 to 1.

    A value outside the range is not applied. The field is framed in red,
    and hovering it says what the game takes. A value already in the file
    that is outside the range is framed the same way. A value that is not
    a number (`@cost`, a variable) is left alone.
  - States are also picked on a map (🗺, in the forms and the wizards):
    the states in their nation's colour, the nation's own bright and the
    picked one lit up, named when zoomed in; the wheel zooms, dragging
    moves, a click picks.
- **Division designer** (at the top of a division template's form): the
  template on the game's grid, five columns of five battalions, the
  support companies beside them and a regimental support company on top of
  each column, each with its icon from the game.
  - Click a place to pick what goes there. The list shows only what the nation
    can use at the start, which is what every nation has plus what the
    build's starting technologies and the nation's own `set_technology`
    unlock (for players with the DLCs picked, when the history gives some
    only inside `has_dlc` checks). It shows only what fits that place: one
    group per column, each support company once, and regimental support on
    the columns it supports. Tick "all" to see the rest greyed out, with
    why: "needs Motorized Infantry", "column 2 is of infantry". Click a
    battalion again to change or remove it; right-click fills or empties its
    column. Each change is one undo step and edits only that line, so
    comments stay.
  - The checks, with the battalions they are about framed in red or
    yellow:
    - battalions the nation cannot use at the start, and the technology
      that unlocks them;
    - places off the grid, or two battalions in one place;
    - a support company among the battalions, or a battalion among the
      support companies;
    - army headquarters units;
    - a support company twice, or two of a kind (two recon companies);
    - regimental support on a column without battalions, or of a group it
      does not support;
    - columns mixing groups;
    - equipment the nation has no model of at the start (nor in its
      stockpile);
    - no battalions at all.

    When the nation's history gives technologies or equipment inside
    `has_dlc` checks, the template is checked for players with and without
    each of those DLCs, and a problem only some of them have says whom:
    "players without No Step Back: Light Tank is not available …". The same
    checks run in **Check** for every template of the project.
  - Stats, with the nation's research and stockpile at the start or with
    all research up to a year (technologies of focuses and events left
    out), for players with every DLC, none, those installed with the game,
    or those ticked (the focus tree view uses the same choice). Research by
    year keeps to the technologies those players see: No Step Back and By
    Blood Alone have their own tank and plane technologies, so light tanks
    of 1940 are the old models without No Step Back and the chassis with
    it.
    - speed, HP, organisation, recovery, recon, suppression, weight, supply
      use, entrenchment;
    - combat width, soft, hard and air attack, defence, breakthrough,
      armour, piercing, hardness;
    - manpower, training time, production cost, and the equipment with the
      model it gets and how much of it the stockpile has.

    Also how many divisions fit each terrain's combat width, and terrain
    bonuses. Hover a stat for how it is worked out. A battalion's attacks
    come from the newest equipment model the nation has, scaled by the
    battalion's own bonuses and the technologies'. The division then sums,
    averages, or takes 40% of the highest plus 60% of the average, as the
    game does. Doctrines, national spirits, generals, experience and the
    nation's own tank designs are left out (tanks use the game's tank
    models without a design).
- **Stockpile** (at the top of a country history's form): the equipment the
  nation has at the start, besides what its divisions carry
  (`add_equipment_to_stockpile`), also the lines inside `has_dlc` checks.
  Each line shows the model with its family's icon, how many (drag to
  change), who made it and which players get it. It also says whether the
  nation can make the model itself, or which technology it would need.
  Divisions draw from the stockpile, so a model the nation cannot make yet
  still equips them, and the division designer counts it. A row adds a
  model picked from every model of the game and the mod (with its family,
  year and DLC). A model only some players get goes inside
  `if = { limit = { has_dlc = ... } }` by itself. A line that gives such a
  model to every player is marked, with a button that puts it inside that
  check. ✖ removes a line. The 🪄 of the field opens a page for one line:
  model, how many, who made it, a design (`variant_name`).
- **Army** (🪖, below Division templates): the divisions a nation starts
  with. The build places none of its own (it once gave every nation a
  militia): a nation starts with the divisions of the order of battle its
  history loads with `oob = "..."`.
  - **Order of battle**: the page shows the file and its line. Without
    one, a button loads a file of the nation's templates
    (`oob = "TAG_content"`), or makes `history/units/TAG_army.txt` and the
    line (after `capital`).
  - **Templates**: those of the file, with their battalions and how many
    divisions are of each. "✏ designer" opens one in the division
    designer, and the designer's "🪖 Place divisions of it on the map"
    comes back with it ready to place. A new template (three infantry
    battalions and engineers) goes into the file and opens in the
    designer. Templates of the nation's other files, which nothing loads,
    are listed with a button that moves them into the order of battle.
  - **Map**: the provinces of the open map, the nation's states bright and
    the rest faded, state borders dark and province borders faint; it
    opens on the nation with its neighbours (wheel: zoom, drag: move).
    Each province with divisions shows how many. Pick a template, then
    click provinces: each click places that many divisions (× 1 to 24),
    named "1. <template>", "2. <template>" ... with 20% experience
    (Trained), 70% of their equipment and all their men
    (`start_manpower_factor = 1`). With no template picked a click (or a
    right-click) lists the divisions of the province.
  - **How a division starts**, as the game does it:
    `start_experience_factor` (0 to 1, else 0) is shown as the game's
    level: Green, Trained from 10%, Regular from 30%, Seasoned from 75%,
    Veterans from 90%. `start_equipment_factor` (else all) is the share of
    its equipment it gets at the start, not from the stockpile; it takes
    the rest from the stockpile when the stockpile has it.
    `start_manpower_factor` is the share of its men it gets; without it
    (as the game's own files are) the men come out of the nation's
    reserves, only as many as there are. A nation of a few villages has a
    few thousand at most, so such divisions start far below strength, and
    the soldiers that fill them up later bring no experience: weaker and
    greener than the file says. New divisions have all three.
  - **Equipment and men the army needs**, above the list: for each kind
    of equipment, the model the divisions get (the newest the nation has
    at the start, with the DLCs picked in the division designer), what
    all the divisions use at full strength, what they are given, what
    they lack (taken from the stockpile) and what the stockpile has
    (orange: not enough). The same for the men (given, or out of the
    reserves). "➕ Add what they lack to the stockpile" puts the shortfall
    in the nation's history (`add_equipment_to_stockpile`, added to a
    plain line of the model when there is one; a model of a DLC inside
    `if = { limit = { has_dlc = ... } }`); "➕ Add a whole army's worth"
    puts in as much again as the whole army uses, for losses and new
    divisions; "Give every division its men" writes
    `start_manpower_factor = 1` where it is missing. Each is one undo step.
  - **Divisions**, by province, in columns: each can be renamed, given
    another template, experience, equipment and men (drag, one undo step),
    moved (📍, then click a province) or removed; "✖ all" clears a
    province. "reserves" marks a division without `start_manpower_factor`
    (click: all its men). Hover a province on the map for its divisions.
  - Checks: a template that is not in the file (the game takes only the
    file's), no template or no location, a location at sea or in a lake,
    in no nation's state or in another nation's, experience, equipment or
    men outside 0 to 1, two divisions of one name, divisions without
    `start_manpower_factor`.
- **Navy** (⚓, below Division templates): the ship designer and the
  fleets a nation starts with.
  - **Ship designer**, as the game's: a design is a hull with a module in
    each slot. Designs are the `create_equipment_variant` lines of the
    nation's history. Pick a hull (those the nation cannot build yet are
    greyed out, with the technology), name the design, and it starts with
    the hull's default modules and the best module the nation has for each
    slot that must be filled. The slots show the module's picture and name;
    click one to pick a module from those the slot takes. The list shows
    only what the nation has researched at the start and what no other
    module of the design rules out; tick "all" to see the rest greyed out,
    with why ("needs Basic Battery", "not with Rapid Fire Guns"). The hull
    can be changed (modules that fit are kept), and a design can be renamed
    (its ships in the fleets follow), duplicated, made obsolete or deleted.
    - Slots, defaults and limits come from the game's and the mod's hulls:
      a model takes the slots of the one it follows where it says
      `inherit`, and `mid_2_custom_slot = mid_1_custom_slot` copies a slot.
    - Checks: a hull or module the nation cannot use at the start, with
      the technology; a module in a slot that does not take it; a slot
      that must be filled left empty; more modules of a kind than the hull
      takes (one radar); modules that cannot be together (a medium battery
      and light guns); a design that is no kind of ship (a heavy hull
      without a battleship's or battlecruiser's armour); two designs of
      one name. A button gives the nation the missing technologies
      (`set_technology`, with those before them on the research screen).
    - What the design is (destroyer, light or heavy cruiser, battleship,
      battlecruiser, carrier, submarine), and its stats: light and heavy
      attack and piercing, torpedoes, depth charges, anti-air; HP, armour,
      reliability, visibility; speed, range, detection, fuel; cost,
      manpower, resources. They are the hull's plus each module's, plus the
      average of the guns' piercing, times the modules' multipliers.
      Refits, design companies, doctrines and technology bonuses are left
      out.
  - **Fleets**: the history/units file the history loads with
    `set_naval_oob` (a button makes `TAG_naval.txt` and the line). Each
    fleet is stationed at a naval base, with task forces of ships. The
    naval bases are those placed in the map editor (Supply level, port
    tool `N`); the lists show the nation's first, then other nations'.
    Fleets and task forces are added with a name and a base; ships are
    added as "n × design", named after it ("Etsch 1", "Etsch 2"), with the
    kind the design makes. Each ship can be renamed, given another design or
    kind, made the pride of the fleet, moved to another task force or
    removed.
    - Checks: a fleet without a naval base; a base or a task force's place
      that is no naval base of the map (with where to place one), or one of
      another nation (it needs docking rights); a ship of a kind not built
      on its hull, of a design the nation does not have (or one on another
      hull of the family), or whose design makes another kind; ships
      without an owner or design; two ships of one name; more than one
      pride of the fleet; empty fleets and task forces.
  - Man the Guns is part of the game now: its technologies, hulls and
    modules are every player's, and the game's own `has_dlc = "Man the
    Guns"` checks always hold.
- **Air** (✈, below Navy): the plane designer and the air wings a nation
  starts with. The plane designer is By Blood Alone's: players without that
  DLC have no airframes or plane modules, but the old plane models
  (`fighter_equipment_1`, `CAS_equipment_1` ...) with upgrades. So each
  design can have a fallback, and the wings come in two files.
  - **Plane designer**, as the game's: a design is an airframe (light,
    light for carriers, medium or heavy) with a module in each slot, the
    `create_equipment_variant` lines of the history inside
    `if = { limit = { has_dlc = "By Blood Alone" } }` (the page puts them
    there). Pick the airframe and the role (fighter, close air support,
    naval bomber, tactical bomber, ...), name it, and it starts with the
    newest main armament of that role and an engine in each slot that must
    be filled. As in the game, the main armament makes the role: each role
    has its own airframe of each generation (`small_plane_cas_airframe_1`),
    and putting bombs on a fighter turns it into a close air support
    plane (the `type` follows). Slots, pickers, "all", renaming (the wings
    and the fallback follow), duplicating, obsolete and deleting work as on
    the Navy page.
    - The airframes of the roles are made by the game's
      `duplicate_archetypes` from those of the light, medium and heavy
      airframe; the app makes them the same way, with the names the game
      gives them.
    - Checks: an airframe or module the nation cannot use at the start (for
      players with By Blood Alone), with the technology; a module a slot
      does not take, an empty slot that must be filled, modules that cannot
      be together; a design outside the DLC check (an error when only
      players without the DLC get it, a warning when every player does); an
      airframe of another role than the main armament makes (with the right
      `type`); more weight than the engines' thrust (an error), or more on
      one of its missions (some modules weigh more on some missions, a
      warning); two designs of one name.
    - Stats: air attack and defence, agility, speed, range; ground attack,
      bombing, naval attack and targeting; thrust and weight, reliability,
      fuel use; cost, resources. The
      airframe's plus each module's, times the modules' multipliers;
      doctrines and technology bonuses are left out.
  - **Fallback**: a design of the same name on an old model, with upgrades
    instead of modules (`type = fighter_equipment_1 upgrades = {
    plane_gun_upgrade = 2 }`), inside `if = { limit = { NOT = { has_dlc =
    "By Blood Alone" } } }`. "Add a fallback" picks the old model of the
    role and generation the nation has (it says which); the model can be
    changed and each upgrade raised or lowered. A design without one is
    noted in the check list (its wings fall back to a plain model).
    - Checks: a model that is no old plane, or that the nation has not
      researched without By Blood Alone (the button gives the old
      technologies); an upgrade the model does not take, or over its
      highest level; a model of another role than the design; a fallback
      outside the check.
  - **Air wings**: two history/units files the history loads with
    `set_air_oob`, one inside the By Blood Alone check and one in its
    `else`, written inside the `if` as the game's histories have it (the
    game drops an `else` that follows the `if`) (a button makes `TAG_air_bba.txt`, `TAG_air_legacy.txt` and
    the lines, or splits a file every player gets). Wings are edited in
    the first: each state with an air base (from the map editor) lists its
    wings, and a wing is an amount of planes of one design, with a name.
    Wings can be renamed, given another design or amount, moved to another
    base or removed.
    - The file without the DLC follows: each wing on its design's fallback,
      or (without one) the old model of the role and generation the nation
      has. When it was changed by hand it is left as it is and marked, with
      a button that rewrites it.
    - Checks: a wing in a state with no air base (with where to place
      one), at another nation's base (it needs the right to use it), or
      more planes at a base than its levels hold; a wing of a design the
      nation does not have, no owner or no amount; airframes in the file
      without the DLC (they do not exist there); a state given twice.
- **Wizards** (🪄 next to a script field, and "Insert…" on the code tab):
  build script without writing it by hand. What they add goes into the
  field where the cursor was (or replaces it, for fields that are a list),
  as one undo step.
  - Effects, conditions and modifiers: the common ones by topic (to take,
    occupy, core or claim a state, ...) with a form for their values (nations, states, ids of the content and the base game
    to pick from), and every effect, trigger and modifier of the game's own
    documentation, filtered by scope or category, with its description and
    example. Conditions can be negated.
  - Wrappers for what was built: `if` (its condition built the same way),
    `hidden_effect`, for a country, a state, every owned state, with a
    chance; OR / NOT / AND for conditions.
  - Fields with their own page: AI weights and mean time to happen (a base,
    then factors or additions when conditions hold), the focus tree's
    `country`, prerequisites and mutually exclusive focuses (tick them), a
    country's politics, popularities (sliders), technologies, ideas,
    research bonus, search filters, a stockpile line, and the battalions,
    support and regimental support companies of a division template (only
    those the nation can use at the start, placed in the free slots, with a
    grid preview).
  - The technologies' page (`set_technology`) lists every technology of
    the game and the mod with its year and research tab, and filters it:
    by name or id, research tab, category (`infantry_weapons`, part of the
    id is enough), years from and to (with or without those that have no
    year), DLC (every player's, only with or only without a DLC, or those
    no player gets now, like the naval tab of before Man the Guns), a
    country (the technologies it starts with in 1936, or those it does
    not: a nation of the mod by its history, a country of the game by the
    game's, the dated blocks of the later starts left out), and ticked or
    not. "Tick the shown" and "untick the shown" act on what the filters
    leave, so "all of Austria's" or "the infantry tab up to 1936" is two
    clicks. Hovering a technology shows its categories, DLC and what it
    unlocks.
  - Every other block of the forms is built whole from a form of its own,
    filled from what the field has (what the form does not know is kept and
    written back after it):
    - a character's leader, general, field marshal, admiral, advisor and
      scientist roles (ideology, traits, skills, slot, cost, conditions) and
      portraits (with the pictures);
    - positions (a tree's continuous focuses and first view, a focus's
      offset with its condition), a technology's folder and path;
    - the subject, relations and variables of a country's history;
    - a scripted localisation's texts, AI strategies, agency upgrade levels;
    - lists to tick (technology and unit categories and types, equipment
      and battalions a technology unlocks, decision targets, peace actions,
      sub-ideologies, the focuses the AI takes, in order);
    - weights (random events of an on action, AI research and ideas,
      resources, equipment a battalion needs);
    - names (first names, surnames, callsigns, faction names);
    - rules of ideas, autonomies and ideologies (yes / no / left out), an
      ideology's colour, and equipment bonuses (stats per equipment).

    Scripts inside them (conditions, effects, modifiers) are built with the
    catalogs: 🪄 next to the part, then ✔ Done.
- **Sprite picker:** every focus icon, idea and event picture, portrait and
  decision icon, of the content and the base game. Filter by words of the
  name or the file, by kind, by whose; a sidebar with the folders of the
  pictures (with counts) narrows it to one folder; thumbnails can be made
  bigger or smaller.
- **Localisation:** every key with its text in two languages, what needs
  one and what uses it. Filter missing, untranslated or unused keys; fill
  from the other language or from the ids.
- **Files:** the package's files as they are, with a text editor; add and
  delete files.

For the whole project:

- **Check:** the content against itself, the map and the base game.
  - Ids that are named but defined nowhere, or defined twice.
  - Focus trees: cycles, overlaps, unreachable focuses, mutually exclusive
    focuses that need each other.
  - Events nothing fires or that cannot be closed; decisions with no
    category.
  - Tags no nation or base-game country has; states and provinces the map
    does not have.
  - Division templates: what the division designer checks.
  - Army (*army*): every order of battle's divisions, as the Army page
    checks them (against the nation that loads the file); files of
    templates that no `oob` or `load_oob` loads (the game never sees
    them); and, as a note, nations whose history has no `oob` (they start
    with no divisions).
  - Navy (*navy*): every nation's ship designs and fleets, as the Navy page
    checks them, and a `set_naval_oob` of a file the content does not
    have.
  - Air (*air*): every nation's plane designs and fallbacks (designs for
    players with By Blood Alone, fallbacks for those without), and its air
    wings against its designs and the map's air bases; a `set_air_oob` of
    a file the content does not have, one of airframes that players
    without By Blood Alone also load, and none for them at all.
  - The stockpile: `add_equipment_to_stockpile` without a type or an
    amount, of what is not equipment, made by a country that does not
    exist.
  - An `else` or `else_if` after an `if` at the top of a nation's history
    (*history*): the game drops it there ("Else/else if not following an
    if" in its error.log); it takes it only inside the `if`. The build
    moves it inside, and "repair" on the Check page does so in the content
    (one undo step).
  - Numbers the game does not allow, as warnings (*values*):
    - fields out of range;
    - in effects and triggers, `set_stability` and `has_stability` 0 to 1,
      `add_stability` and `add_popularity` -1 to 1, `set_popularities` 0
      to 100;
    - whole numbers of research slots, building slots, manpower, stockpile
      amounts and event days;
    - `add_building_construction` with a level over the building's cap
      (infrastructure 5) or a building the game does not have.

    The wizards check the same numbers before they add a line or fill a
    form: a leader's skills, a subject's freedom, a focus's position.
  - DLC, from the game's own files: research folders shown only with a
    DLC or only without it (No Step Back's tanks, By Blood Alone's planes),
    the technologies in them, and the models those technologies unlock.
    Man the Guns is part of the game now: every player has it.
    - `has_dlc` of a name that is no DLC (an error with the right name
      for a near miss: "No Step back" → "No Step Back");
    - a technology (`set_technology`) or a model (`add_equipment_to_stockpile`,
      `create_equipment_variant`) only some players have, given outside a
      `has_dlc` check for it;
    - the content's technologies in a research folder that does not exist,
      or that only some players see (a note).

    The `has_dlc` checks around a line count: those of the `if`,
    `else_if` and `else` it is in, and a focus's `allow_branch`, a
    decision's `allowed` or `available` and the like.
  - Missing localisation, icons, pictures and texture files; script that
    does not parse.

  Errors, warnings and notes can be filtered by kind and package and copied
  as text. Every row opens the file at its line, or the definition.
- **Import:** from the base game, a mod of the game's mod folder (folders
  and `.zip`), any folder, a bundle, or another nation.
  - Browse by kind, filter, and pick definitions (or all of a tag's) with a
    preview.
  - The plan then follows what they use (events, ideas, characters,
    scripted effects, traits, ...). It renames the source's tag to the
    nation's and ids that would clash. It brings their localisation, and
    the sprites and pictures of a mod; the base game's stay in the game.
  - Country history loses what is about the source's map (capital, states,
    armies).
  - Everything is shown before it is applied, as one undo step.
- **Export and copy:** a nation's content as a bundle (`.zip`, imported
  again elsewhere), as plain files in a folder (existing files are never
  overwritten), or into another nation (a plan, as in Import).
- **Test play and errors:**
  1. Check.
  2. Save and build the mod.
  3. Play from the build window.
  4. Read the game's `error.log`. It is read again when it changes, shows
     the lines about the content (or all of them), and each line links to
     the file it names.
- **Tools:**
  - Find & replace (words or regex, over all text).
  - Rename or delete an id everywhere, and see where it is named.
  - Change a nation's tag throughout.
  - The graph of event chains.
  - A gallery of focus icons, idea and event pictures and portraits, the
    base game's too.
  - The sprite check: named but undefined, defined without a picture,
    unused.
  - Statistics.
  - What changed since the last save, with snapshots to restore.
  - What a mod has in common with the content (ids, files, tags).
  - An icon maker (focus, idea, event, portrait) that writes the DDS and its
    sprite.

The base game is read in the background when the screen opens (about 1 s).
Until then, checks and imports that need it wait.

## Building the mod

"Build mod" in the editor (or `--build-mod PROJECT [--game DIR] [--mods
DIR]`) makes the mod from the saved map in about 4 s. Its inputs are the
project's `provinces.bmp` and `provinces.json`, and the elevation (heights in
metres) and land cover (CLC+ Backbone classes) it refers to. Without them,
the heights come from the imported heightmap or are flat, and the land is
grass where no terrain is painted. Palettes, texture sizes, name
lists and the game version come from the HOI4 install. The terrain colours,
tree type and city look are sampled from the vanilla Alps around Innsbruck.

It writes `<mods>/<folder>/` and `<mods>/<folder>.mod`:

- **map:** heightmap and terrain (with the editor's painting and imported
  heightmap), normals, `rivers.bmp` (the editor's rivers), trees, cities, colour maps, `definition.csv`, buildings, unit /
  weather / supply positions, `railways.txt` and `supply_nodes.txt` (as laid
  in the editor, else a minimum spanning tree over neighbouring states
  around impassable borders, and a hub in the main victory point of each
  state), adjacencies, strategic regions, and `ambient_object.txt` (the
  frame only if the option is on, ocean sounds over this map's sea zones, no
  Great Wall, and the editor's objects, with the entities they need in
  `gfx/entities/mod_helper_ambient.asset`);
- **history:** states (owner, occupier as `controller`, cores as
  `add_core_of`, category, manpower, buildings, victory points, the
  editor's naval bases), countries (with no divisions: those are the
  content's, from the Army page);
- **countries:** one per nation, with tag, colour unlike its neighbours',
  procedural flag, and name lists by language;
- **other:** the bookmark, localisation, a tutorial pointed at this map (the
  vanilla one crashes on a new map), `descriptor.mod`, thumbnail, README;
- **content:** the files of `<map>/content/`, `shared` first, then each
  nation's.
  - A nation's `history/countries/` file goes after the history the build
    writes for it, so what it says wins. An `else` / `else_if` that follows
    an `if` there is moved inside the `if`, where the game takes it.
  - A script or localisation file with the name of a built one is written
    under another name (`x_TYR.txt`), so both are read.
  - Any other file replaces the built one (a flag of your own), with a line
    in the build log.
  - Content for a tag no nation has stops the build.
  - Unsaved content is saved first.

Values set in the editor win over derived ones. The new folder is built next
to the old one and swapped in at the end, so a failed build keeps the last
good mod. A folder of that name is only replaced if mod-helper (or the old
`make_map_mod.py`) made it.

Then the mod is checked against the game's rules (also `--validate-mod
MOD_DIR [GAME_DIR]`): bitmap sizes and formats, provinces against
`definition.csv`, heightmap water (the sea under the water plane, land above
it, lakes either way), states and regions covering every province
once, positions inside their provinces, adjacencies, railways (levels 1–5),
naval bases (coastal, levels 1–10), `rivers.bmp` (one pixel wide, markers,
no blocks), building slots, countries (tags, files, colours, flags, history, units, names, localisation),
bookmark, tutorial ids, the entities and places of the ambient objects,
and the `.mod` path.

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

All layers are drawn in one world CRS, the map's projection (EPSG:3035, LAEA
Europe, for the Tyrol map). On a plain raster, nothing is reprojected. A raster's georeference is read in GDAL's order:
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
| `osm.rs` | three-pass `.osm.pbf` extraction of boundaries, rivers and lake outlines + cache |
| `vector.rs` | projected boundaries and water with Douglas–Peucker LODs, line styles per admin level and kind of water |
| `layers.rs` | layer config (persisted) and runtime state, pixel ↔ world placement |
| `view.rs` | camera, raster/vector drawing, scale bar |
| `app.rs` | UI (egui/eframe 0.37, wgpu) |
| `project.rs` | the project's settings (`project.json`), old project files, the recent projects |
| `bundle.rs` | project files: the ZIP of settings and map, the working folder, packing what changed, converting old projects |
| `zip.rs` | ZIP archives: stored and deflated entries, raw copies, no zip64 |
| `mapproj.rs` | where a map lies: plain raster or map projection, sizes HOI4 takes, changing the projection |
| `newproject/` | new projects: blank maps; imports of the base game or a mod (`hoi4.rs`) |
| `welcome.rs` | the welcome screen, the new project form, the Map projection window |
| `editor/model.rs` | map data (id tables, label bitmap, per-province statistics and borders), load / save with backups |
| `editor/doc.rs` | open map with transactions and undo / redo (data snapshots + pixel change logs) |
| `editor/ops.rs` | all edit operations: paint, merge, divide, generate, dissolve, states, nations, zones, adjacencies, inaccessible zones |
| `editor/kmeans.rs`, `editor/density.rs` | weighted k-means with the box limit and the border warp; the terrain rules and density model |
| `editor/validate.rs` | HOI4 map checks, run on a snapshot in the background |
| `editor/render.rs` | the overlay as a tile source (fill per province by colour mode, graded borders, selection) |
| `editor/mod.rs`, `editor/build.rs` | editor UI; Mod details; Build mod on a worker thread |
| `editor/pending.rs` | edits in the properties panel: applied field by field, never lost |
| `editor/straits.rs` | straits carried along by pixel edits; "Fix straits" |
| `editor/terrain_ui.rs` | the build's terrain rules in the editor: rules editor, colour mode *terrain type*, counts per rule |
| `editor/texture_ui.rs` | the Terrain level: painting `terrain.bmp`, import / export of `terrain.bmp` and `heightmap.bmp`, their colour modes |
| `editor/launcher.rs` | the game launcher: mod list, `dlc_load.json`, starting the game |
| `editor/from_image.rs` | "Province from image": colour channel of an image laid on the map, preview |
| `editor/lasso.rs` | the Lasso tool: the pixels inside a loop, new provinces and states from it, drawing it |
| `editor/cut.rs` | the Cut tool: the pixels of a line, what it divides, cutting provinces and states along it |
| `editor/rivers.rs`, `editor/rivers_ui.rs` | rivers as lines → `rivers.bmp` pixels (junctions, lakes, sea), its checks, rivers from the heights; the Rivers level |
| `editor/supply.rs`, `editor/supply_ui.rs` | the supply network: railway routing, automatic railways and hubs, ports, the supply estimate; the Supply level |
| `modbuild/mod.rs` | the mod build: bitmaps, positions, railways, states, countries, bookmark, localisation, tutorial |
| `modbuild/validate.rs` | checks of a built mod |
| `modbuild/buildings.rs` | what the game allows of buildings: level caps, shared building slots, coastal ones, those that exclude each other, the slots and island caps of each state category (from the game, else vanilla's) |
| `modbuild/meta.rs` | mod metadata (stored in `provinces.json`), `descriptor.mod`, thumbnail |
| `modbuild/terrain.rs` | the terrain rules: kinds of land, counting them per province, the terrain of a province |
| `modbuild/bitmaps.rs` | `terrain.bmp` and `heightmap.bmp` as the build makes them, painted terrain and imported heightmaps, vanilla's terrain indices and palette |
| `modbuild/img.rs` | BMP / DDS / TGA I/O, Pillow-equivalent resampling, distance transform |
| `modbuild/vanilla.rs`, `countries.rs`, `rail.rs` | samples from the game; tags, colours, flags; railway graph |
| `content/script.rs`, `loc.rs` | the game's script (lossless parse, spans for edits) and localisation files |
| `content/kinds.rs`, `about.rs` | the kinds of content: where they live, how definitions are found, their fields, templates; what each field does, at length |
| `content/store.rs` | the content packages in memory: files, definitions, undo / redo, save with backups |
| `content/source.rs`, `import.rs` | the base game, mods, folders and bundles as sources; import plans (follow, rename, localisation, sprites) |
| `content/focus.rs`, `drawio.rs` | focus trees: layout, checks, play, edits; trees from draw.io |
| `content/docs.rs`, `wizard.rs`, `shapes.rs` | the game's documentation of effects, triggers and modifiers; the wizards' catalog, roles of fields, and how built script is merged in; the forms of the fields built whole (read, write, what is missing) |
| `content/validate.rs`, `errlog.rs` | the content checks; the game's `error.log` |
| `content/units.rs` | land units of the game and the mod (battalions, equipment, modules, technologies, research folders, terrain widths): what a nation can use at the start (its stockpile too, for players with which DLCs), the checks of a division template, its stats |
| `content/limits.rs` | the numbers the game allows: fields of the definitions, values in effects and triggers, parts of the wizards' forms, building levels |
| `content/navy.rs` | the navy: ship hulls and their slots, modules, designs (checks, stats, what kind of ship), fleets and their checks against the designs and the map's naval bases |
| `content/air.rs` | the air force: airframes and their role families (`duplicate_archetypes`), the role a main armament makes, plane designs (checks, stats, thrust and weight) and their fallbacks on old models with upgrades, air wings and their checks against the designs and the map's air bases, the wings without By Blood Alone |
| `content/army.rs` | the army: a nation's `oob` line and file, its templates, the divisions (read, written, named), their checks against the file's templates and the map's provinces, the files nothing loads |
| `content/dlc.rs` | DLC: the names `has_dlc` takes, what a player needs (or must not have) for a part of the game, the `has_dlc` checks around a line of script (an `else` inside the `if` too), DLCs every player has now |
| `content/refactor.rs`, `gfx.rs`, `zip.rs`, `build.rs` | renames, deletes, find & replace; sprites and pictures; bundles (in memory, with `zip.rs`); the content in the built mod |
| `content/ui/` | the screen: nations, forms and code editor, wizards, the sprite picker and the state map, the division designer, the stockpile, the army (with a map of the provinces), navy and air pages, the focus tree canvas, check, import / export, test play, tools |
| `editor/content_link.rs` | what the content screen asks of the map: nations, tags, states, the build |
| `models/pdxbin.rs`, `mesh.rs`, `anim.rs` | the game's binary `.mesh` and `.anim` files: meshes, skeletons, skinning, locators, animation samples |
| `models/catalog.rs` | the game's, the DLCs' and the mod's `pdxmesh`, `animation` and `entity` definitions (clones, states, attachments), where they come from, case-blind file lookup |
| `models/render.rs`, `math.rs` | a small software renderer (depth buffer, perspective-correct mipmapped textures, cut-outs, sun light) and the 3D maths |
| `models/mod.rs` | a model ready to draw: entity + mesh + textures + animations + attached entities; posing and drawing it; the pictures of the browser and the map |
| `editor/objects_ui.rs` | the Objects level: browser, preview and its window, the scene on the map, placing, moving, turning, the panel |
| `models/import/` | new models: glTF / GLB and OBJ / MTL readers, a `.mesh` of Paradox's exporter, the game's space and tangents, textures in the game's layout (DDS with mipmaps), the `.gfx` / `.asset` definitions |
| `editor/model_import_ui.rs` | the "Import a model…" window: options per material, the preview, writing into a content package |
| `modbuild/ambient.rs` | the objects in the build: `ambient_object.txt` blocks and the entities they need |

A hidden layer is `LayerState::Unloaded` (`LayerState::residency` says when
to read or drop one, each frame) and keeps its size and georeference in
`Layer::known`. Rasters sit behind `Arc<RwLock<Raster>>` and tiles are keyed by
`(layer, level, x, y)`, so editing tools can change pixels and invalidate only
the affected tiles.

## Limitations

- The app has one project open at a time and keeps its map in memory
  (about 0.3 GB for the 4608 × 2816 map with undo). Two copies of the app
  must not open the same project file (they share its working folder).
- Project files are ZIP archives without zip64: each file in them and the
  whole archive must stay under 4 GB.
- A plain raster has no place on the Earth: OSM layers stay empty on it,
  and rasters only line up by their pixel size. HOI4's own maps are not true
  projections, so an imported vanilla map stays a plain raster unless you
  give it one.
- "Remove from play" (lake provinces) and "Make lake" are untested in the
  game (the built files pass the checks and match vanilla's lakes).
- Ambient objects are only partly tested in the game (the turn's direction
  and the height above the ground are checked). Still to check:
  - tilt and roll, taken to be mirrored like the turn, and the order of
    the three rotations (a guess);
  - whether `use_animation` plays a clone's `default_state` (vanilla never
    sets one on a clone);
  - whether, under the sea and lakes, the game's terrain is the seabed (as
    the build assumes) or the water plane. If it is the water plane, ships
    stand up to one unit too high; lower them with the lift.
- The model preview is a software renderer: no normal maps, specular or
  shadows, and textures at most 512 px.
- Imported models are static. Bones and animations of a glTF are not
  read. A `.mesh` keeps its bones, but its animations (`.anim`) are not
  imported, and the game draws a skinned model only while it plays one.
  Textures are written uncompressed: four times the size of the game's
  DXT5. Draco or meshopt compressed glTFs, texture transforms and
  diffuse transparency (the game's shader draws no holes) are not
  supported. Imported models were checked with the game's own files
  (shader, meshes, textures) and through the build's checks. They have not
  been looked at in the game yet.
- The supply estimate leaves out motorization, damage, weather and fleets;
  the game's numbers will differ somewhat.
- Rasters are loaded fully into RAM in their native format. The 40000² CLC+
  map takes about 1.6 GB.
- Band-separate (planar) multi-band TIFFs and RLE4 BMPs are not supported.
