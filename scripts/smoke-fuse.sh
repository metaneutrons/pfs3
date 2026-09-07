#!/usr/bin/env bash
# Mount smoke test for pfs3-fuse.
#
# The FUSE driver has no unit tests, and a compile only proves it links. This
# mounts a real image and compares what the kernel serves through the driver
# against what the CLI reads from the same image directly, so the two halves
# have to agree rather than merely run.
#
# Needs /dev/fuse and an unprivileged FUSE mount, which GitHub's ubuntu runners
# provide. Run from the repository root:
#
#   scripts/smoke-fuse.sh
#
# PFS3_BIN_DIR points at the directory holding the two release binaries,
# default ./target/release. PFS3_SMOKE_KEEP=1 leaves the work directory behind.
set -euo pipefail

fixture=crates/libpfs3/tests/fixtures/small.hdf
[ -f "$fixture" ] || { echo "missing fixture: $fixture" >&2; exit 1; }

case "$(uname -s)" in
  Linux)  unmount() { fusermount3 -u "$1" 2>/dev/null || fusermount -u "$1" 2>/dev/null || umount "$1" 2>/dev/null; } ;;
  Darwin) unmount() { umount "$1" 2>/dev/null || diskutil unmount force "$1" >/dev/null 2>&1; } ;;
  *)      echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac

work=$(mktemp -d)
mnt="$work/mnt"; mkdir -p "$mnt"
fuse_pid=""
cleanup() {
  [ -n "$fuse_pid" ] && kill "$fuse_pid" 2>/dev/null || true
  unmount "$mnt" || true
  if [ "${PFS3_SMOKE_KEEP:-0}" = 1 ]; then echo "kept: $work"; else rm -rf "$work"; fi
}
trap cleanup EXIT

# Overridable, so a caller with a different CARGO_TARGET_DIR can point at the
# binaries it actually built instead of whatever sits in ./target.
bindir=${PFS3_BIN_DIR:-$(pwd)/target/release}
cli="$bindir/pfs3"
fuse="$bindir/pfs3-fuse"
for b in "$cli" "$fuse"; do
  [ -x "$b" ] || { echo "missing binary: $b — build them, or set PFS3_BIN_DIR" >&2; exit 1; }
done

# Work on a copy throughout. The read-write half must not be able to touch the
# fixture the rest of the test suite depends on.
img="$work/small.hdf"
cp "$fixture" "$img"
fixture_before=$(sha256sum "$fixture" 2>/dev/null | cut -d" " -f1 || shasum -a 256 "$fixture" | cut -d" " -f1)

fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }
ok()   { echo "  ok  $*"; }

# Mount and wait for the kernel to actually serve the mountpoint. A bounded
# wait with a hard abort: a mount that never appears is a failure, not
# something to keep polling for.
mount_and_wait() {
  "$fuse" "$img" "$mnt" "$@" >"$work/fuse.log" 2>&1 &
  fuse_pid=$!
  for _ in $(seq 1 50); do
    if [ -n "$(ls -A "$mnt" 2>/dev/null)" ]; then return 0; fi
    kill -0 "$fuse_pid" 2>/dev/null || { cat "$work/fuse.log" >&2; fail "the driver exited before the mount appeared"; }
    sleep 0.2
  done
  cat "$work/fuse.log" >&2
  fail "the mountpoint stayed empty for ten seconds"
}

unmount_and_wait() {
  unmount "$mnt" || true
  for _ in $(seq 1 50); do
    if [ -z "$(ls -A "$mnt" 2>/dev/null)" ]; then fuse_pid=""; return 0; fi
    sleep 0.2
  done
  fail "the mountpoint was still served after ten seconds"
}

echo "read-only mount"
mount_and_wait

# The name set the driver serves must equal the name set the CLI reads. Both
# sides are derived rather than hardcoded, so a change to the fixture cannot
# make this pass by accident.
"$cli" ls "$img" | awk 'NF>3 && $1 ~ /^(FILE|DIR)$/ {print $NF}' | sort > "$work/cli-names"
# find rather than `ls | grep`: the mount can legitimately serve the virtual
# .Trashcan directory, which the image itself does not contain.
find "$mnt" -mindepth 1 -maxdepth 1 -not -name '.Trashcan' -printf '%f\n' | sort > "$work/fuse-names"
diff -u "$work/cli-names" "$work/fuse-names" || fail "the directory listing differs"
ok "listing matches, $(wc -l < "$work/cli-names" | tr -d ' ') entries"

# Every file the listing names has to read back identically through both paths.
checked=0
while read -r name; do
  [ -f "$mnt/$name" ] || continue
  "$cli" cat "$img" "$name" > "$work/via-cli"
  cp "$mnt/$name" "$work/via-fuse"
  cmp "$work/via-cli" "$work/via-fuse" || fail "$name differs between the CLI and the mount"
  checked=$((checked + 1))
done < "$work/cli-names"
[ "$checked" -gt 0 ] || fail "no file was compared, the test proved nothing"
ok "$checked file(s) byte-identical through both paths"

# A directory in the listing must be a directory on the mount.
while read -r name; do
  if "$cli" ls "$img" | awk -v n="$name" '$1=="DIR" && $NF==n {found=1} END{exit !found}'; then
    [ -d "$mnt/$name" ] || fail "$name is a directory to the CLI but not on the mount"
    ok "$name is a directory on both sides"
  fi
done < "$work/cli-names"

# The size the driver reports has to match the size the CLI reports.
size_cli=$("$cli" ls "$img" | awk '$1=="FILE" {print $3; exit}')
name_cli=$("$cli" ls "$img" | awk '$1=="FILE" {print $NF; exit}')
size_fuse=$(wc -c < "$mnt/$name_cli" | tr -d ' ')
[ "$size_cli" = "$size_fuse" ] || fail "size of $name_cli: CLI says $size_cli, mount says $size_fuse"
ok "size of $name_cli agrees: $size_cli bytes"

# Read-only really is read-only.
# The redirect failure is bash's own message, so the whole attempt is wrapped
# rather than just the command inside it.
if ( echo x > "$mnt/should-not-work" ) 2>/dev/null; then
  fail "a read-only mount accepted a write"
fi
ok "the read-only mount refused a write"

unmount_and_wait
ok "unmounted"

echo "read-write mount"
mount_and_wait --write

payload="pfs3-fuse smoke $(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '%s\n' "$payload" > "$mnt/smoke.txt" || fail "writing through the mount failed"
mkdir "$mnt/SmokeDir" || fail "mkdir through the mount failed"
ok "wrote a file and created a directory"

unmount_and_wait
ok "unmounted"

# The decisive check: the CLI, reading the image directly with the mount gone,
# has to see what was written through the driver.
"$cli" ls "$img" | awk '$NF=="smoke.txt"' | grep -q smoke.txt || fail "smoke.txt is not in the image after unmounting"
"$cli" ls "$img" | awk '$1=="DIR" && $NF=="SmokeDir"' | grep -q SmokeDir || fail "SmokeDir is not in the image after unmounting"
"$cli" cat "$img" smoke.txt > "$work/written"
printf '%s\n' "$payload" > "$work/expected"
cmp "$work/expected" "$work/written" || fail "smoke.txt content differs from what was written"
ok "the write reached the on-disk filesystem and reads back identically"

# The copy has to have changed, otherwise nothing was written at all and every
# check above passed against an untouched image.
if cmp -s "$fixture" "$img"; then
  fail "the copy is byte-identical to the fixture, so nothing was actually written"
fi
ok "the image really changed"

# And the fixture the rest of the test suite depends on must be untouched.
fixture_after=$(sha256sum "$fixture" 2>/dev/null | cut -d" " -f1 || shasum -a 256 "$fixture" | cut -d" " -f1)
[ "$fixture_before" = "$fixture_after" ] || fail "the fixture was modified"
ok "the fixture is unchanged"

echo "SMOKE PASS"
