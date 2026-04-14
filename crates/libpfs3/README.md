# libpfs3

A pure Rust library for reading, writing, and manipulating [PFS3 (Professional File System III)](https://aminet.net/package/disk/misc/pfs3aio) volumes — the most popular filesystem for Amiga hard drives.

## Features

- Parse PFS3 on-disk structures (rootblock, directory entries, anodes) in big-endian format
- Read files, list directories, follow softlinks
- Write files, create directories, delete entries, rename, set protection bits
- Format new PFS3 volumes
- Filesystem consistency checker with optional repair
- RDB (Rigid Disk Block) partition table parsing with auto-detection
- SUPERINDEX support for large (multi-GB) volumes
- Deldir (trash) access for recovering deleted files
- LRU block cache
- No OS dependencies — works on any platform Rust supports

## Usage

```rust
use libpfs3::volume::Volume;

// Open a PFS3 disk image
let mut vol = Volume::open("disk.hdf", 0, false)?;

// List root directory
for entry in vol.list_dir("/")? {
    println!("{} {:>8} {}", 
        if entry.is_dir() { "DIR " } else { "FILE" },
        entry.file_size(),
        entry.name);
}

// Read a file
let data = vol.read_file("S/Startup-Sequence")?;
println!("{}", String::from_utf8_lossy(&data));
```

### Writing

```rust
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

let vol = Volume::open("disk.hdf", 0, true)?;  // writable = true
let mut w = Writer::open(vol)?;

w.write_file("hello.txt", b"Hello from Rust!")?;
w.create_dir("NewDir")?;
```

### RDB disk images

```rust
use libpfs3::volume::Volume;

// Auto-detect first PFS3 partition in an RDB image
let mut vol = Volume::open_auto("amiga_drive.hdf", 0, Some("DH0"), false)?;
```

### Protection bits

```rust
use libpfs3::util;

// Parse and format Amiga protection bits
let prot = util::parse_amiga_protection(0x0F, "hsparwed").unwrap();
assert_eq!(util::amiga_protection_string(prot), "hsparwed");

// Convert between Unix mode and Amiga protection
let mode = util::amiga_protection_to_mode(prot, false);
```

## Compatibility

Tested against volumes created by:

- [pfs3aio](https://github.com/tonioni/pfs3aio) — original AmigaOS driver
- [Coffin OS](https://www.apollo-accelerators.com/wiki/doku.php/start) R65 — 32GB multi-partition RDB images
Supports MODE_SUPERINDEX, MODE_SUPERDELDIR, MODE_DELDIR, and MODE_LARGEFILE.

## License

LGPL-3.0-or-later
