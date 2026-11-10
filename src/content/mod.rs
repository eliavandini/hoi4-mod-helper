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
//! | [`zip`] | bundles |
//! | [`refactor`] | renames, deletes, find & replace, tag renames across the project |
//! | [`errlog`] | the game's error.log, pointed back to the content |
//! | [`gfx`] | pictures of sprites (DDS, TGA, PNG) as textures |

pub mod about;
pub mod build;
pub mod docs;
pub mod drawio;
pub mod errlog;
pub mod focus;
pub mod gfx;
pub mod import;
pub mod kinds;
pub mod loc;
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
