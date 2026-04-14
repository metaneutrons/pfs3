# pfs3 — Amiga PFS3 Filesystem Tools & FUSE Driver

[![CI](https://github.com/metaneutrons/pfs3/actions/workflows/ci.yml/badge.svg)](https://github.com/metaneutrons/pfs3/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/libpfs3.svg)](https://crates.io/crates/libpfs3)
[![docs.rs](https://docs.rs/libpfs3/badge.svg)](https://docs.rs/libpfs3)
[![License: LGPL-3.0-or-later](https://img.shields.io/badge/license-LGPL--3.0--or--later-blue.svg)](https://www.gnu.org/licenses/lgpl-3.0)
[![AUR](https://img.shields.io/aur/version/pfs3)](https://aur.archlinux.org/packages/pfs3)

A pure Rust implementation of the [Professional File System III (PFS3)](https://aminet.net/package/disk/misc/pfs3aio) for Amiga disk images. Read, write, format, check, and mount PFS3 volumes on modern systems.

## Features

- **Read access** — list directories, read files, extract entire volumes, follow softlinks
- **Write support** — create files, directories, delete entries, rename, format new volumes
- **Filesystem checker** — validate on-disk consistency with optional repair
- **FUSE driver** — mount PFS3 images as native filesystems (macOS via macFUSE, Linux via FUSE)
- **Amiga protection bits** — full HSPARWED support via CLI, `chmod`, and extended attributes
- **RDB auto-detection** — automatically finds PFS3 partitions in Rigid Disk Block images
- **Deldir / .Trashcan** — browse and recover deleted files
- **Real device support** — works on raw partitions (`/dev/sdX`, `/dev/rdiskN`)
- **SUPERINDEX support** — handles large volumes (multi-GB) with superindex anode trees
- **Pure Rust** — no C dependencies (except FUSE for mounting)

## Installation

This project provides two packages:

- **`pfs3`** — CLI tools (no dependencies, static binary)
- **`pfs3-fuse`** — FUSE driver (requires libfuse3 on Linux or macFUSE/FUSE-T on macOS)

### Pre-built binaries

Download from [GitHub Releases](https://github.com/metaneutrons/pfs3/releases). Available for Linux (x86_64, aarch64) and macOS (Intel, Apple Silicon).

### Homebrew (macOS)

```bash
brew tap metaneutrons/tap
brew install pfs3
```

### Arch Linux (AUR)

```bash
# CLI tools
yay -S pfs3

# FUSE driver
yay -S pfs3-fuse
```

### Debian / Ubuntu

Download `.deb` packages from [GitHub Releases](https://github.com/metaneutrons/pfs3/releases):

```bash
sudo dpkg -i pfs3_*.deb
sudo dpkg -i pfs3-fuse_*.deb    # optional, pulls in libfuse3
```

### From source

```bash
# CLI tools only
cargo install --path .

# FUSE driver (requires libfuse3-dev on Linux or macFUSE on macOS)
cargo install --path crates/pfs3-fuse
```

Requires Rust 1.85+ (edition 2024).

## CLI Usage

### Inspect a volume

```bash
pfs3 info disk.hdf
pfs3 info amiga_drive.hdf -p DH0       # select partition by name
pfs3 partitions amiga_drive.hdf         # list all RDB partitions
```

### List files

```bash
pfs3 ls disk.hdf                        # root directory
pfs3 ls disk.hdf S                      # subdirectory
pfs3 ls disk.hdf -p DH0 C              # partition + path
```

Output includes Amiga protection bits:

```
Type     Protect            Size Date                 Name
------------------------------------------------------------------------
FILE     ----r-ed            976 1978-01-01 00:05:08  Wait
FILE     -s--rwed             14 2021-06-16 19:27:30  WOMyFiles
DIR      ----rwed                2023-07-30 18:05:40  C
```

### Read and extract files

```bash
pfs3 cat disk.hdf S/Startup-Sequence    # print to stdout
pfs3 extract disk.hdf / -o ./output     # extract everything
pfs3 extract disk.hdf Libs -o ./libs    # extract a directory
```

### Write files

```bash
pfs3 write disk.img localfile.txt RemoteName.txt
pfs3 mkdir disk.img NewDir
pfs3 write disk.img data.bin NewDir/data.bin
pfs3 rm disk.img OldFile.txt
```

### Set protection bits

```bash
pfs3 protect disk.img myfile.txt rwed       # absolute: set exactly these bits
pfs3 protect disk.img myfile.txt "+sp"      # add script + pure flags
pfs3 protect disk.img myfile.txt -- "-wd"   # remove write + delete
pfs3 protect disk.img myfile.txt hsparwed   # set all 8 bits
```

Protection bits follow Amiga conventions:

| Bit | Name    | Meaning                                    |
|-----|---------|--------------------------------------------|
| H   | Hidden  | File is hidden from directory listings      |
| S   | Script  | File is an executable script                |
| P   | Pure    | Program can be made resident (re-entrant)   |
| A   | Archive | File has been modified since last backup     |
| R   | Read    | File can be read                            |
| W   | Write   | File can be written                         |
| E   | Execute | File can be executed                        |
| D   | Delete  | File can be deleted                         |

> Note: RWED use inverted logic on disk (bit set = denied). The CLI and xattr interface abstract this away — lowercase letters mean "granted".

### Check and repair

```bash
pfs3 check disk.hdf
pfs3 check disk.hdf --repair
```

### Format a new volume

```bash
pfs3 mkfs new.img --name "MyDisk" --size-mb 100
pfs3 mkfs /dev/sda3 --name "WorkDisk"
```

### Other commands

```bash
pfs3 tune disk.hdf --name "NewName"     # rename volume
pfs3 deldir disk.hdf                    # list deleted files
```

## FUSE Driver

Mount any PFS3 image as a native filesystem:

```bash
mkdir -p /tmp/pfs3
pfs3-fuse disk.hdf /tmp/pfs3 --auto-unmount

# Use it like any filesystem
ls /tmp/pfs3/
cat /tmp/pfs3/S/Startup-Sequence
cp /tmp/pfs3/C/Dir ~/amiga-dir

umount /tmp/pfs3
```

### Read-write mode (experimental)

```bash
pfs3-fuse disk.hdf /tmp/pfs3 --auto-unmount --write

echo "hello" > /tmp/pfs3/test.txt
mkdir /tmp/pfs3/NewDir
mv /tmp/pfs3/old.txt /tmp/pfs3/new.txt
rm /tmp/pfs3/unwanted.txt
```

> ⚠️ Read-write mode is experimental. PFS3 does not support journaling or atomic updates — this is a limitation of the on-disk format, same as the original AmigaOS driver. Use only on copies of disk images, never on originals.

### RDB disk images

```bash
# Auto-detect first PFS3 partition
pfs3-fuse amiga_drive.hdf /tmp/pfs3

# Manual byte offset
pfs3-fuse drive.img /tmp/pfs3 --offset 258048
```

### Amiga protection bits via extended attributes

When mounted via FUSE, Amiga protection bits are accessible through the `user.amiga.protection` extended attribute. This preserves all 8 bits (HSPARWED) losslessly — unlike `chmod` which can only express RWED.

```bash
# Read protection bits
getfattr -n user.amiga.protection /tmp/pfs3/myfile
# user.amiga.protection="--p-rwed"

# Set protection bits (same syntax as CLI)
setfattr -n user.amiga.protection -v "hsparwed" /tmp/pfs3/myfile
setfattr -n user.amiga.protection -v "+sp" /tmp/pfs3/myfile
setfattr -n user.amiga.protection -v "-wd" /tmp/pfs3/myfile

# List all xattrs
getfattr -d /tmp/pfs3/myfile
```

`chmod` also works and maps to RWED bits, preserving any HSPA flags that were previously set:

```bash
chmod 644 /tmp/pfs3/myfile    # sets rw-d, preserves hspa
chmod 755 /tmp/pfs3/myfile    # sets rwed, preserves hspa
```

### Virtual .Trashcan

If the volume has a deldir (PFS3 trash), it appears as a virtual `.Trashcan` directory at the mount root:

```bash
ls /tmp/pfs3/.Trashcan/
cat /tmp/pfs3/.Trashcan/deleted_file.txt
```

## Project Structure

```
pfs3/
├── crates/
│   ├── libpfs3/              ← Core library (no OS dependencies)
│   │   ├── src/
│   │   │   ├── ondisk/         — On-disk structures (rootblock, direntry), BE parsing
│   │   │   ├── volume.rs       — Volume open, RDB detection, file/dir access
│   │   │   ├── anode.rs        — Anode lookup, chain traversal, SUPERINDEX
│   │   │   ├── dir.rs          — Directory entry parsing
│   │   │   ├── bitmap.rs       — Block bitmap allocation
│   │   │   ├── format.rs       — Filesystem formatter (mkfs)
│   │   │   ├── writer.rs       — File/dir write, delete, rename, protection
│   │   │   ├── rdb.rs          — Rigid Disk Block partition table parser
│   │   │   ├── cache.rs        — LRU block cache
│   │   │   ├── io.rs           — BlockDevice trait + file/partition impl
│   │   │   ├── error.rs        — Error types
│   │   │   └── util.rs         — Datestamp, charset, protection bit conversion
│   │   └── tests/
│   │       ├── integration.rs  — 76+ tests against real PFS3 images
│   │       └── fixtures/
│   │           ├── small.hdf   — Generated test image (320KB)
│   │           └── pfs.7z      — Real PFS3 image from AmiFUSE (8MB)
│   └── pfs3-fuse/            ← FUSE driver binary
│       └── src/
│           ├── main.rs         — FUSE filesystem implementation
│           └── types.rs        — Inode management, volume access
├── src/                      ← CLI tool binary
│   ├── main.rs
│   ├── bin/
│   │   └── gen_fixtures.rs   — Test fixture generator
│   └── cmd/
│       ├── info.rs, ls.rs, cat.rs, extract.rs
│       ├── check.rs, mkfs.rs, write.rs
│       ├── protect.rs, tune.rs
│       └── mod.rs
├── Cargo.toml
└── LICENSE                   — LGPL-3.0-or-later
```

## Test Images

- **`small.hdf`** (320KB) — Generated by `gen_fixtures` using the PFS3 formatter. Contains files, directories, and various edge cases.
- **`pfs.7z`** → `pfs.hdf` (8MB) — Created by the real PFS3 driver (`pfs3aio`) running under m68k emulation. Sourced from the [AmiFUSE project](https://github.com/reinauer/AmiFUSE). Automatically extracted at test time via `sevenz-rust`.

All core tests run against both images to ensure compatibility with real PFS3 on-disk structures.

## Compatibility

Tested against:

- Volumes created by the original [pfs3aio](https://github.com/tonioni/pfs3aio) driver
- [Coffin OS](https://www.apollo-accelerators.com/wiki/doku.php/start) R65 32GB disk images (multi-partition RDB, SUPERINDEX, deldir)
- Volumes with MODE_SUPERINDEX, MODE_SUPERDELDIR, MODE_DELDIR, MODE_LARGEFILE

## References

- [pfs3aio](https://github.com/tonioni/pfs3aio) — Original PFS3 AmigaOS driver source by Michiel Pelt
- [AmiFUSE](https://github.com/reinauer/AmiFUSE) — FUSE driver using m68k emulation of real Amiga FS handlers

## License

LGPL-3.0-or-later
