# Security policy

## Supported versions

Only the latest released version is supported. Fixes go into a new release
rather than into patches for older tags.

## Reporting a vulnerability

Report privately through GitHub's private vulnerability reporting:

<https://github.com/metaneutrons/pfs3/security/advisories/new>

Please do not open a public issue for a security problem.

You get an acknowledgement within 72 hours and an assessment within 14 days. If
the report is confirmed, you are told when a fix is planned and are credited in
the advisory unless you ask otherwise.

## Scope

This project reads and writes on-disk filesystem structures, so the interesting
attack surface is a **malicious or corrupt disk image**. Parsing a crafted image
must not lead to memory unsafety, an unbounded allocation or a hang. Reports of
that kind are in scope, as is anything that lets a mounted image escape the
mount point through the FUSE driver.

Out of scope: data loss caused by the experimental write path on an image the
user opened read-write on purpose. That limitation is documented in the README.

## Fuzzing

`crates/libpfs3/fuzz/` holds `cargo-fuzz` targets for the root block, directory
entries, volume opening and the writer. Findings from those are welcome through
the same private route.
