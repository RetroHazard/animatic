# Animatic developer documentation

These pages describe how Animatic is built: architecture, storage and network contracts,
development, releases and the rename engine's Rust API. A stored setting or a Rust API is not
necessarily a desktop feature; the historical forum audit is evidence, not a product
specification.

## User guide

The user guide is the [wiki](https://github.com/RetroHazard/animatic/wiki): installation, the
command line and data locations, every page and task of the application, the settings
reference, the rename language, troubleshooting and feature parity with AOM. Its source is the
[`wiki/`](../wiki) folder of this repository; changes go through pull requests and are
published by the wiki workflow (see [Wiki publishing](development.md#wiki-publishing)). Release
archives carry the same pages in their `docs/` folder.

## Develop Animatic

| Guide | Read it for |
|---|---|
| [Architecture](architecture.md) | Crate dependencies, threads (including the updater thread), data flows, invariants, extension points and where each user workflow is implemented |
| [Storage and network contracts](storage-and-network.md) | Schema, queue ordering and failure states, AniDB protocol handling, the GitHub update transport and Rust API entry points |
| [Development](development.md) | Toolchain, checks, native smoke verification, CI, wiki publishing and contribution/privacy rules |
| [Releases](releases.md) | Versioning and changelog, release tool, workflows, maintainer procedure, archive contents, checksum verification and the in-app update contract |
| [Rename language Rust API](rename-api.md) | Planning a rename from Rust, against fixed objects or the library database, and custom data sources |
| [Roadmap](roadmap.md) | Remaining features, derived from the feature parity page: milestones, ordering, constraints, dependencies and open decisions |

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
