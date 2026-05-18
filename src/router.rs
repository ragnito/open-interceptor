//! Re-export Router from the domain layer for compatibility.
//!
//! New code should import from `crate::domain::router` directly.

pub use crate::domain::router::Router;
