//! Core library of Animatic, the successor of AniDB O'Matic.

pub mod db;
pub mod error;
pub mod external;
pub mod file_manager;
pub mod format;
pub mod hash;
pub mod legacy;
pub mod library;
pub mod rename;
pub mod settings;
pub mod stats;
pub mod store;
pub mod sync;

pub use error::{Error, Result};
