//! AniDB API clients used by Animatic.
//!
//! * [`udp`] — the UDP API (login, file identification, mylist management).
//! * [`commands`] — typed wrappers for the UDP commands.
//! * [`http`] — the HTTP API (full anime records), title dump and images.
//!
//! Both APIs require a client registered at <https://anidb.net/software/add>.

pub mod commands;
pub mod error;
pub mod http;
pub mod udp;

pub use error::{AnidbError, Result};
