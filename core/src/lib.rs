//! Scanning and analysis engine for Ableton Organizer.
//!
//! Design rule: when in doubt, keep the file. Every heuristic here errs on the
//! side of reporting something as "used".
pub mod analysis;
pub mod cache;
pub mod daw;
pub mod duplicates;
pub mod model;
pub mod plugins;
pub mod projects;
pub mod samples;
pub mod system;
pub mod trash;

pub use model::*;
