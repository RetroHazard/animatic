//! AniDB API clients used by Animatic.
//!
//! This crate speaks the protocols and nothing else: it has no database, no command queue and
//! no dependency on the rest of the workspace. It classifies errors (see [`error`]) and keeps
//! only in-memory protocol state (a one-time re-login/retry, a short server backoff and a ban
//! latch per client) plus the process-wide request spacing in [`limits`], whose state it saves
//! through a [`limits::LimitStore`] the caller supplies. Persistent retry and cooldown policy is
//! not here: `animatic-core` provides persistence and the command executor, and the
//! application's worker decides how to retry, block or pause.
//!
//! * [`udp`] — the UDP API (login, file identification, MyList management): protocol version 3,
//!   one blocking client per thread, tag matching, optional API-key encryption, compressed
//!   replies and a single automatic re-login/retry. The bound local port is never replaced by
//!   another one.
//! * [`limits`] — one [`limits::Limits`] per process, shared by every client: UDP packets at
//!   least 2 s apart (4 s once a burst exceeds five), HTTP requests at least 2 s apart, send
//!   times saved before each request, and the escalating backoff after an unanswered `AUTH`.
//! * [`commands`] — typed wrappers for the UDP commands. Success codes are decoded strictly;
//!   only an explicit "no such" code means absence, any other unexpected or malformed reply is
//!   an error.
//! * [`http`] — the HTTP API (full anime records), the streamed and validated anime title dump
//!   (bounded by [`http::TitlesLimits`]) and image download. The anime API is plain HTTP and
//!   sends no credentials; the dump and images use HTTPS.
//! * [`error`] — [`AnidbError`] with `is_transient`, `is_fatal` and `retry_after`, which callers
//!   use to choose between blocking a command, pausing all network use, or stopping the queue.
//!
//! The UDP API is not protected by TLS: without an API key the login, including the password, is
//! sent unencrypted, and even the encrypted mode has no message authentication.
//!
//! Both APIs require a client registered at <https://anidb.net/software/add>. Tests in this
//! workspace use local fixtures and must never contact production AniDB.

pub mod commands;
pub mod error;
pub mod http;
pub mod limits;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod udp;

pub use error::{AnidbError, Result};
