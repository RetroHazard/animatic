# Animatic documentation

These guides describe the implementation in this repository. A stored setting or a Rust API
is not necessarily a desktop feature. Unsupported and compatibility-only behavior is labeled
explicitly; the historical forum audit is evidence, not a product specification.

## Use Animatic

| Guide | Read it for |
|---|---|
| [Getting started](getting-started.md) | Download a release or build, first run, data locations, portable setup and CLI reference |
| [User guide](user-guide.md) | Library views, hashing, identification, MyList edits, scans, migration, exports and external tools |
| [Settings reference](settings-reference.md) | JSON fields, defaults, UI controls, active versus stored-only options and recovery rules |
| [Rename language](rename-language.md) | Expressions, functions, object properties, working patterns, validation and safe application |
| [Troubleshooting](troubleshooting.md) | Backups, recovery, synchronization failures, filesystem problems and privacy-safe support |

For an existing AOM library, start with the migration section in the user guide. Keep the
original databank and all runtime output outside the source checkout. Before online work,
review imported settings and use a registered AniDB client identifier.

## Develop Animatic

| Guide | Read it for |
|---|---|
| [Architecture](architecture.md) | Crate dependencies, threads, data flows, invariants and extension points |
| [Storage and network contracts](storage-and-network.md) | Schema, queue ordering and failure states, protocol handling and Rust API entry points |
| [Development](development.md) | Toolchain, checks, native smoke verification, CI and contribution/privacy rules |
| [Releases](releases.md) | Versioning and changelog, release tool, workflows, maintainer procedure, archive contents and checksum verification |
| [Feature parity](feature-parity.md) | Desktop workflows versus core-only capabilities, compatibility differences and open gaps |

Generate API documentation with `cargo doc --workspace --no-deps`; the local entry points
are `target/doc/animatic_core/index.html` and `target/doc/animatic_anidb/index.html`.
The [agent handoff](../AGENTS.md) supplies repository-specific engineering rules.

## Evidence and provenance

The [AOM forum audit](aom-forum-audit.md) preserves source links, historical comparisons,
verification records and limits of the available research. It is not a completed crawl or
a promise that every reported AOM failure has been reproduced in Animatic.

Windows and Linux are supported targets. Local runtime verification and CI build/test
coverage are different evidence, and a successful release run is not runtime evidence either;
neither implies production AniDB validation. Never put
real settings, credentials, library records or AOM artifacts into documentation examples.
