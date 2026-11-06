//! Core library of Animatic, the successor of AniDB O'Matic.
//!
//! This crate owns the application's state and rules; the `animatic` crate only adds a window
//! and a background worker around it. It depends on `animatic-anidb` (typed protocol clients
//! and records) and not on the UI. The command executor [`sync::execute`] lives here: it runs a
//! queued command against live UDP/HTTP clients and applies the reply to the database. This
//! crate provides persistence, the executor and the queue/cooldown storage; it does not run the
//! queue by itself. The application worker owns the policy (when to run, and how to react to a
//! failed command: retry, block, or pause all network use).
//!
//! # Modules
//!
//! * [`atomic`] — crash-safe file replacement (temporary file, flush, rename) used by the
//!   hash and statistics exports.
//! * [`db`] — SQLite connection setup (WAL), idempotent schema creation, the command-queue
//!   upgrade, and the persisted sync cooldown. [`db::Db`] is one connection; open one per thread.
//! * [`limits`] — saved AniDB limit state: the send times and `AUTH` backoff step behind the
//!   process-wide spacing, and the attempt-based guards (title dump once a day, minimum age per
//!   anime and per file).
//! * [`store`] — write side: AniDB records, MyList rows, hashed local files, and the atomic
//!   title-dump merge (`Db::import_title_dump`, implemented in [`store`]).
//! * [`library`] — read side: display types, queries and the immutable
//!   [`library::LibraryProjection`] snapshot used by the UI.
//! * [`maintenance`] — library safety: online backup, validated restore, integrity check and
//!   compaction (blocking; never on the UI thread).
//! * [`sync`] — the durable command outbox: [`sync::Command`], priorities, latest-intent edit
//!   merging, blocked/deferred/failed states, pending-intent overlays and [`sync::execute`].
//! * [`hash`] — one-pass ED2K, CRC32, MD5 and SHA-1 hashing with pause/cancel hooks.
//! * [`file_manager`] — side-effect-free scan planning plus hash-and-register and
//!   [`file_manager::move_file`].
//! * [`legacy`] — decryption and per-collection import of an AniDB O'Matic databank.
//! * [`rename`] — the AOM-compatible rename language and validated path planning.
//! * [`settings`] — serializable settings, backup-aware loading and AOM settings import.
//! * [`secrets`] — the system keystore (Windows Credential Manager, Linux Secret Service) for the
//!   AniDB password and API keys, the in-memory test store and the visible settings-file
//!   fallback used by [`settings::Settings::save`].
//! * [`stats`] — library statistics and XML/INI rendering.
//! * [`external`] — launching external players and AVDump without a shell.
//! * [`update`] — GitHub release check, verified download and in-place installation of a newer
//!   stable release (separate [`update::UpdateError`]; never touches the AniDB cooldown).
//! * [`instance`] — single-instance handoff: a launch with media files forwards them to the
//!   running instance that has the same database open (loopback socket and lock file).
//! * [`shell_integration`] — the optional per-user "Hash with Animatic" file-menu entry
//!   (Windows registry or Linux desktop file) that only an Options save creates.
//! * [`mod@format`] — shared size/date/duration formatting.
//! * [`ui_state`] — pure rules for the saved window geometry, per-table column layout and
//!   sorting, and status row colours.
//!
//! # Guarantees worth knowing
//!
//! * MyList edits are merged per entry and saved together with the optimistic local change in
//!   one write transaction; queue ids are never reused, so a stale acknowledgement cannot
//!   remove newer intent. Delivery to AniDB is at-least-once.
//! * Applying an API reply layers the *readable* queued local MyList edits of that entry on top
//!   of the remote values. Rows this version cannot read cannot be overlaid, so remote values
//!   may overwrite what they represented. A malformed reply is a protocol error, never evidence
//!   that something is absent.
//! * Rows this version cannot read stay in the queue as blocked entries instead of being
//!   deleted.
//! * [`file_manager::move_file`] never leaves the library pointing at a missing file and never
//!   deletes the only copy: a rename across volumes is copied, verified against the stored ED2K,
//!   swapped into place and recorded before the source is removed. It never replaces an
//!   existing target on the cross-volume path; a same-volume rename is not an atomic
//!   no-overwrite operation.
//! * [`file_manager::clear_emptied_folders`] is the only folder removal: after Animatic's own move,
//!   strictly beneath the matching manager root, one non-recursive `remove_dir` per completely
//!   empty folder, never the root itself.
//! * Exports ([`stats::LibraryStats::write`], [`db::Db::export_local_hashes`]) replace their
//!   target through [`atomic::write_atomic`]: a failed write leaves the previous file intact.
//!   [`db::Db::library_stats`] reads every figure in one read transaction and returns an error
//!   instead of zeros when a query fails.
//! * Credentials in [`settings::Settings`] are stored as plain text.
//!
//! Table-level and protocol-level reference: `docs/storage-and-network.md`; threads and data
//! flow: `docs/architecture.md` in the repository.

pub mod atomic;
pub mod db;
pub mod error;
pub mod external;
pub mod file_manager;
pub mod format;
pub mod hash;
pub mod instance;
pub mod legacy;
pub mod library;
pub mod limits;
pub mod maintenance;
pub mod rename;
pub mod secrets;
pub mod settings;
pub mod shell_integration;
pub mod stats;
pub mod store;
pub mod sync;
pub mod ui_state;
pub mod update;

pub use error::{Error, Result};
