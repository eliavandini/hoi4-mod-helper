# mod-helper

Map tools for the Tyrol / Trentino HOI4 total conversion:

1. a **layer viewer**: the rasters in `../maps/processed maps` and the
   administrative boundaries, rivers and lakes in `../maps/osm regions`, stacked as layers that
   can be toggled, reordered, moved, restyled and imported;
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

## Running

```sh
cargo run --release                     # opens mod-helper.project.json (created on first run)
cargo run --release -- other.project.json
cargo run --release -- --editor         # start in the map editor
cargo run --release -- --content        # start in the nation content screen
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
  refinery, silo, radar, rocket site, reactor), resources, victory points.
- Nation: name, tag, colour, language (name lists), ruling ideology,
  settlement rank, capital, whether it starts with a militia.
- Zone: name. Adjacency: name, type, sea province crossed, rule, ends.

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
- *From the heights* follows the water down `dem_section.tif` to the sea or
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
  TIFF, grey) is used instead of the one from `dem_section.tif`. The build
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
not on a coast. Click an issue
to go there.

**Saving and building.** Ctrl+S (or 💾) writes `provinces.bmp` and
`provinces.json` (and the Terrain level's `terrain_paint.bmp` and
`heightmap_import.bmp` when there are any). The previous files are copied to `backups/<time>/` first
(UTC time of the save); the last 10 are kept. Ids are renumbered 1..n on save
if anything was removed. "Backups…" lists them with their age and province
count; "Load" puts one in the editor as one edit (Undo goes back). Nothing is
written until you save. The mod details and the map frame setting stay as they are.
Reload and Open… ask before they drop unsaved changes.
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
`content/.backups/<time>/` first. Packages are named by tag, so a nation
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
- **Wizards** (🪄 next to a script field, and "Insert…" on the code tab):
  build script without writing it by hand. What they add goes into the
  field where the cursor was (or replaces it, for fields that are a list),
  as one undo step.
  - Effects, conditions and modifiers: the common ones by topic with a form
    for their values (nations, states, ids of the content and the base game
    to pick from), and every effect, trigger and modifier of the game's own
    documentation, filtered by scope or category, with its description and
    example. Conditions can be negated.
  - Wrappers for what was built: `if` (its condition built the same way),
    `hidden_effect`, for a country, a state, every owned state, with a
    chance; OR / NOT / AND for conditions.
  - Fields with their own page: AI weights and mean time to happen (a base,
    then factors or additions when conditions hold), the focus tree's
    `country`, prerequisites and mutually exclusive focuses (tick them),
    a country's politics, popularities (sliders), technologies, ideas,
    research bonus, search filters, and the battalions of a division
    template (placed in the free slots, with a grid preview).
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

"Build mod" in the editor (or `--build-mod [MAP_DIR] [--game DIR] [--mods
DIR]`) makes the mod from the saved map in about 4 s. Inputs in the map
folder: `provinces.bmp` + `provinces.json`, `dem_section.tif` (heights) and
`landcover_smooth.bmp` (land cover classes). Palettes, texture sizes, name
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
- **history:** states (category, manpower, buildings, victory points, the
  editor's naval bases), countries and their militia;
- **countries:** one per nation, with tag, colour unlike its neighbours',
  procedural flag, and name lists by language;
- **other:** the bookmark, localisation, a tutorial pointed at this map (the
  vanilla one crashes on a new map), `descriptor.mod`, thumbnail, README;
- **content:** the files of `<map>/content/`, `shared` first, then each
  nation's.
  - A nation's `history/countries/` file goes after the history the build
    writes for it, so what it says wins.
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
| `osm.rs` | three-pass `.osm.pbf` extraction of boundaries, rivers and lake outlines + cache |
| `vector.rs` | projected boundaries and water with Douglas–Peucker LODs, line styles per admin level and kind of water |
| `layers.rs` | layer config (persisted) and runtime state, pixel ↔ world placement |
| `view.rs` | camera, raster/vector drawing, scale bar |
| `app.rs` | UI (egui/eframe 0.37, wgpu) |
| `project.rs` | project files and default layer discovery |
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
| `modbuild/meta.rs` | mod metadata (stored in `provinces.json`), `descriptor.mod`, thumbnail |
| `modbuild/terrain.rs` | the terrain rules: kinds of land, counting them per province, the terrain of a province |
| `modbuild/bitmaps.rs` | `terrain.bmp` and `heightmap.bmp` as the build makes them, painted terrain and imported heightmaps, vanilla's terrain indices and palette |
| `modbuild/img.rs` | BMP / DDS / TGA I/O, Pillow-equivalent resampling, distance transform |
| `modbuild/vanilla.rs`, `countries.rs`, `rail.rs` | samples from the game; tags, colours, flags; railway graph |
| `content/script.rs`, `loc.rs` | the game's script (lossless parse, spans for edits) and localisation files |
| `content/kinds.rs` | the kinds of content: where they live, how definitions are found, their fields, templates |
| `content/store.rs` | the content packages in memory: files, definitions, undo / redo, save with backups |
| `content/source.rs`, `import.rs` | the base game, mods, folders and bundles as sources; import plans (follow, rename, localisation, sprites) |
| `content/focus.rs`, `drawio.rs` | focus trees: layout, checks, play, edits; trees from draw.io |
| `content/docs.rs`, `wizard.rs` | the game's documentation of effects, triggers and modifiers; the wizards' catalog, roles of fields, and how built script is merged in |
| `content/validate.rs`, `errlog.rs` | the content checks; the game's `error.log` |
| `content/refactor.rs`, `gfx.rs`, `zip.rs`, `build.rs` | renames, deletes, find & replace; sprites and pictures; bundles; the content in the built mod |
| `content/ui/` | the screen: nations, forms and code editor, wizards and the sprite picker, the focus tree canvas, check, import / export, test play, tools |
| `editor/content_link.rs` | what the content screen asks of the map: nations, tags, states, the build |
| `models/pdxbin.rs`, `mesh.rs`, `anim.rs` | the game's binary `.mesh` and `.anim` files: meshes, skeletons, skinning, locators, animation samples |
| `models/catalog.rs` | the game's, the DLCs' and the mod's `pdxmesh`, `animation` and `entity` definitions (clones, states, attachments), where they come from, case-blind file lookup |
| `models/render.rs`, `math.rs` | a small software renderer (depth buffer, perspective-correct mipmapped textures, cut-outs, sun light) and the 3D maths |
| `models/mod.rs` | a model ready to draw: entity + mesh + textures + animations + attached entities; posing and drawing it; the pictures of the browser and the map |
| `editor/objects_ui.rs` | the Objects level: browser, preview and its window, the scene on the map, placing, moving, turning, the panel |
| `modbuild/ambient.rs` | the objects in the build: `ambient_object.txt` blocks and the entities they need |

Rasters sit behind `Arc<RwLock<Raster>>` and tiles are keyed by
`(layer, level, x, y)`, so editing tools can change pixels and invalidate only
the affected tiles.

## Limitations

- The map editor edits one map project at a time and keeps it in memory
  (about 0.3 GB for the 4608 × 2816 map with undo).
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
- The supply estimate leaves out motorization, damage, weather and fleets;
  the game's numbers will differ somewhat.
- Rasters are loaded fully into RAM in their native format. The 40000² CLC+
  map takes about 1.6 GB.
- Band-separate (planar) multi-band TIFFs and RLE4 BMPs are not supported.
