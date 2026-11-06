//! Twins: finds duplicate and look-alike images in a folder.
//!
//! - `scan` walks the folder on background threads and scores every look-alike pair. That is the
//!   slow part and it only runs once.
//! - `review` is the keep/delete state, with no UI in it. The threshold is just a filter over the
//!   scores, so moving it is instant and never rescans.
//! - `panel` is the egui screen: two pictures side by side and the keep/delete buttons.
//!
//! The folder doesn't reach outside itself. The host passes in how to decode a picture and how
//! to delete a file (see `Options` and `Panel::new`).

pub mod fingerprint;
pub mod panel;
pub mod review;
pub mod scan;

pub use fingerprint::Picture;
pub use panel::{Event, Panel};
pub use scan::Options;

/// Where the threshold starts. Resized and re-encoded copies score 97 and up, a burst shot or an
/// added logo in the 90s, and the same design in another colour around 80.
pub const DEFAULT_THRESHOLD: f32 = 0.9;
