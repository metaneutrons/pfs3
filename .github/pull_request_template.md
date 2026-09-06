## What changes

<!-- One or two sentences. The title follows Conventional Commits and becomes
     the subject line on main, because merges are squash merges. -->

## Why

<!-- The reason, not a restatement of the diff. -->

## How it was checked

<!-- Which commands were run, against which images, on which platform. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo nextest run --workspace --all-features --locked`
- [ ] `cargo deny check`

## On-disk format

- [ ] This change does not alter what is written to disk
- [ ] It does, and it is covered by tests against both `small.hdf` and `pfs.hdf`
