//! Tool #3, nation content: focus trees, events, decisions, ideas,
//! characters and the rest of what makes a country in HOI4, per nation of
//! the map (and for the mod as a whole), with the checks the game would
//! make, imports from the base game, other mods and other nations, and
//! bundles to take a nation's content elsewhere.
//!
//! | module | what |
//! |---|---|
//! | [`script`] | Paradox script read with byte spans, edits that keep the rest of a file |
//! | [`loc`] | localisation files |
//! | [`kinds`] | the kinds of content: folders, shapes, fields, localisation, templates |
//! | [`about`] | what the fields do in the game, at length, with a line of each |
//! | [`store`] | the project's content in memory: packages, undo, save |
//! | [`focus`] | focus trees: positions, the game's rules, checks, layout, edits |
//! | [`source`] | the base game, mods, folders, nations and bundles to import from |
//! | [`import`] | importing (what comes, renames, localisation, pictures), bundles |
//! | [`validate`] | the checks of the content against the game and the map |
//! | [`units`] | battalions, equipment and technologies for the division designer: what a nation can use, checks, stats |
//! | [`navy`] | ship hulls, modules and designs (checks, stats), fleets against the designs and the map's naval bases |
//! | [`air`] | airframes, plane modules and designs (roles, thrust and weight, checks), fallbacks for players without By Blood Alone, air wings at the map's air bases |
//! | [`army`] | the order of battle a nation starts with: its `oob` line, division templates and divisions placed in the map's provinces, their checks |
//! | [`limits`] | the numbers the game allows: fields, values in effects and triggers, building levels |
//! | [`dlc`] | DLC names, what of the game needs or excludes a DLC, the `has_dlc` checks around a line |
//! | [`zip`] | bundles |
//! | [`refactor`] | renames, deletes, find & replace, tag renames across the project |
//! | [`errlog`] | the game's error.log, pointed back to the content |
//! | [`gfx`] | pictures of sprites (DDS, TGA, PNG) as textures |

pub mod about;
pub mod build;
pub mod dlc;
pub mod docs;
pub mod drawio;
pub mod errlog;
pub mod focus;
pub mod gfx;
pub mod import;
pub mod kinds;
pub mod limits;
pub mod loc;
pub mod navy;
pub mod air;
pub mod army;
pub mod refactor;
pub mod shapes;
pub mod wizard;
pub mod script;
pub mod source;
pub mod store;
pub mod ui;
pub mod units;
pub mod validate;
pub mod zip;

#[cfg(test)]
mod tests;
