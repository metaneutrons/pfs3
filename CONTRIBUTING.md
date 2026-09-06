# Contributing

## Setup

```bash
brew install lefthook gitleaks   # or: mise use -g lefthook gitleaks
lefthook install
```

The toolchain comes from `rust-toolchain.toml` and is the only source of the
Rust version. Do not pin a second one in a workflow or locally.

For the FUSE driver you additionally need `libfuse3-dev` on Linux or macFUSE on
macOS. Without it `cargo build -p pfs3-fuse` fails in the `fuser` build script,
and the rest of the workspace still builds.

## Branches and commits

Branch names: `feat/<short-topic>`, `fix/<short-topic>`, `docs/<short-topic>`.

[Conventional Commits](https://www.conventionalcommits.org/) are binding.
release-please derives the next version and the changelog from the commit
types, so a commit outside the scheme produces a wrong version or a missing
changelog entry.

Permitted types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore`, `revert`. Scope optional and lower case. Breaking
changes through `!` after the type and `BREAKING CHANGE:` in the body.

**The pull request title matters most.** Merges are squash merges and the title
becomes the subject line on `main`, so that title is what determines the
version. The CI job `commit-hygiene` checks it.

No AI attribution trailers, in commits or in the pull request body. If you use
an assistant, switch its attribution off at the source.

## Local checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features --locked
cargo deny check
```

The hooks run a subset of this: formatting and the staged-file guards before a
commit, clippy and the secret scan before a push. Everything else runs in CI.

If a hook gets in your way, say so in an issue rather than reaching for
`--no-verify`. A hook that people bypass costs the fast checks too.

## Tests

`cargo nextest run --workspace` runs against two images: `small.hdf`, generated
by this project's own formatter, and `pfs.hdf`, produced by the original PFS3
driver under m68k emulation and extracted from `pfs.7z` at test time. A change
to the on-disk code has to pass against both.

The test files under `crates/libpfs3/tests/` are organised by concern:
`read`, `write`, `format`, `rdb`, `deldir`, `corrupt`, `fault` and `stress`.
Put a new test with the concern it belongs to.

## Pull requests

CI has to be green before a merge. `CI Success` is the required check and it
aggregates the others, so a failing matrix leg shows up there.
