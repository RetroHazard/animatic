## Summary

<!-- What changes and why. Link issues with "Fixes #N". -->

## Verification

<!-- Report only what you ran, with the platform. Delete lines that do not apply. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --release -p animatic`
- [ ] UI change exercised in the native application with isolated, offline settings:
- [ ] Platforms exercised (Windows / Linux):

## Checklist

Full list: [Contribution checklist](https://github.com/RetroHazard/animatic/blob/main/docs/development.md#contribution-checklist).

- [ ] No private data: no settings, databases, exports, logs, real paths, hashes, AniDB ids or credentials; fixtures are synthetic.
- [ ] No AOM files, databanks or reverse-engineering artifacts.
- [ ] Tests assert behavior; none contact AniDB.
- [ ] Schema changes are idempotent and keep existing databases working.
- [ ] Affected documentation is updated, and user-visible changes are noted under Unreleased in `CHANGELOG.md`.
