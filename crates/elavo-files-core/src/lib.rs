//! The parts of Elavo Files that do not need GTK: how names sort, how
//! sizes and dates read, the places in the sidebar, the attributes asked of
//! GIO for each file, back/forward history, Finder's names for copies and
//! aliases, the clipboard formats, the Trash, Elavo application bundles, and the
//! file operations themselves.
//!
//! Kept free of widgets so it can move into `elavo-folio/crates` with the
//! app and be shared with Folio's desktop later.

pub mod clipboard;
pub mod eapp;
pub mod entry;
pub mod format;
pub mod history;
pub mod naming;
pub mod ops;
pub mod places;
pub mod sort;
pub mod trash;
pub mod zoom;
