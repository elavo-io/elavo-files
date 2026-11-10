//! The parts of Elavo Files that do not need GTK: how names sort, how
//! sizes and dates read, the places in the sidebar, the attributes asked of
//! GIO for each file, and back/forward history.
//!
//! Kept free of widgets so it can move into `elavo-folio/crates` with the
//! app and be shared with Folio's desktop later.

pub mod entry;
pub mod format;
pub mod history;
pub mod places;
pub mod sort;
pub mod zoom;
