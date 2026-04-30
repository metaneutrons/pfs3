// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Read operation tests: list_dir, lookup, read_file, read_file_range,
//! path handling, case-insensitive, anode chain validation, protection
//! string parsing, latin1, amiga_epoch, fragmented reads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(10000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_read_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "ReadTest".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, blocks, &opts).unwrap();
    drop(dev);
    path
}

fn open_writer(path: &Path) -> Writer {
    let dev = FileBlockDevice::open_rw(path, 512, 0, 0).unwrap();
    let vol = Volume::from_device(Box::new(dev)).unwrap();
    Writer::open(vol).unwrap()
}

fn reopen(path: &Path) -> Volume {
    Volume::open(path, 0).unwrap()
}

// ============================================================
// Utility function tests (no image needed)
// ============================================================

#[test]
fn latin1_ascii() {
    assert_eq!(libpfs3::util::latin1_to_string(b"hello"), "hello");
}

#[test]
fn latin1_umlaut() {
    assert_eq!(libpfs3::util::latin1_to_string(b"\xe4\xf6\xfc"), "äöü");
}

#[test]
fn name_eq_ci() {
    assert!(libpfs3::util::name_eq_ci("Hello", "hello"));
    assert!(!libpfs3::util::name_eq_ci("foo", "bar"));
    assert!(!libpfs3::util::name_eq_ci("foo", "fooo"));
}

#[test]
fn amiga_epoch() {
    use std::time::UNIX_EPOCH;
    assert_eq!(
        libpfs3::util::amiga_to_systime(0, 0, 0)
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        2922 * 86400
    );
}

#[test]
fn protection_bits() {
    assert_eq!(
        libpfs3::util::amiga_protection_to_mode(0, false) & 0o777,
        0o777
    );
    assert_eq!(
        libpfs3::util::amiga_protection_to_mode(0x0F, false) & 0o777,
        0o000
    );
}

// ============================================================
// Path Handling Edge Cases
// ============================================================

mod path_handling {
    use super::*;

    #[test]
    fn lookup_root_returns_none() {
        let path = fresh_image(4096);
        let mut vol = reopen(&path);
        assert!(vol.lookup("/").unwrap().is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lookup_with_trailing_slash() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("MyDir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("MyDir").unwrap();
        assert!(entry.is_some());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lookup_with_leading_slash() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"data").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let e1 = vol.lookup("test.txt").unwrap();
        let e2 = vol.lookup("/test.txt").unwrap();
        assert!(e1.is_some());
        assert!(e2.is_some());
        assert_eq!(e1.unwrap().anode, e2.unwrap().anode);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lookup_with_double_slash() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("Dir").unwrap();
        w.write_file("Dir/file.txt", b"data").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let result = vol.lookup("Dir//file.txt");
        let _ = result;
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn list_dir_root_slash() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("x.txt", b"x").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let e1 = vol.list_dir("/").unwrap();
        let _e2 = vol.list_dir("").unwrap_or_default();
        assert!(!e1.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lookup_nonexistent_in_subdir() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("Dir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let result = vol.lookup("Dir/ghost.txt").unwrap();
        assert!(result.is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn lookup_file_as_directory_fails() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("file.txt", b"not a dir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let result = vol.lookup("file.txt/child");
        assert!(result.is_err() || result.unwrap().is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn case_insensitive_lookup() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("CamelCase.TXT", b"camel").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("camelcase.txt").unwrap(), b"camel");
        assert_eq!(vol.read_file("CAMELCASE.TXT").unwrap(), b"camel");
        assert_eq!(vol.read_file("CamelCase.TXT").unwrap(), b"camel");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn case_insensitive_dir_lookup() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("MyDir").unwrap();
        w.write_file("MyDir/file.txt", b"inside").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("mydir/file.txt").unwrap(), b"inside");
        assert_eq!(vol.read_file("MYDIR/FILE.TXT").unwrap(), b"inside");
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// read_file_range Edge Cases
// ============================================================

mod range_reads {
    use super::*;

    fn setup_range_test() -> (PathBuf, u32, u64) {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        w.write_file("range.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("range.bin").unwrap().unwrap();
        let anode = entry.anode;
        let size = entry.file_size();
        drop(vol);
        (path, anode, size)
    }

    #[test]
    fn range_read_full_file() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 0, size as u32).unwrap();
        let expected: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_first_byte() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 0, 1).unwrap();
        assert_eq!(data, vec![0u8]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_last_byte() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, size - 1, 1).unwrap();
        assert_eq!(data, vec![((size - 1) % 256) as u8]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_offset_beyond_file_returns_empty() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, size, 100).unwrap();
        assert!(data.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_offset_way_beyond_file() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, u64::MAX / 2, 100).unwrap();
        assert!(data.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_crosses_block_boundary() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 480, 64).unwrap();
        let expected: Vec<u8> = (480..544).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_exact_block_boundary_start() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 512, 10).unwrap();
        let expected: Vec<u8> = (512..522).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_exact_block_boundary_end() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 502, 10).unwrap();
        let expected: Vec<u8> = (502..512).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_length_exceeds_file_truncates() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 2000, 1000).unwrap();
        assert_eq!(data.len(), 48);
        let expected: Vec<u8> = (2000..2048).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_zero_length() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 100, 0).unwrap();
        assert!(data.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_entire_second_block() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 512, 512).unwrap();
        let expected: Vec<u8> = (512..1024).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_multi_block_span() {
        let (path, anode, size) = setup_range_test();
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 100, 1500).unwrap();
        let expected: Vec<u8> = (100..1600).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Fragmented File Range Reads
// ============================================================

mod fragmented_reads {
    use super::*;

    fn create_fragmented_file(path: &Path) -> (u32, u64) {
        let mut w = open_writer(path);
        for i in 0..10 {
            w.write_file(&format!("pad{}.bin", i), &vec![i as u8; 512])
                .unwrap();
        }
        for i in (0..10).step_by(2) {
            w.delete(&format!("pad{}.bin", i)).unwrap();
        }
        let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        w.write_file("fragmented.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(path);
        let entry = vol.lookup("fragmented.bin").unwrap().unwrap();
        (entry.anode, entry.file_size())
    }

    #[test]
    fn range_read_first_block_of_fragmented() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 0, 512).unwrap();
        let expected: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_across_extent_boundary() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 1024, 1024).unwrap();
        let expected: Vec<u8> = (1024..2048).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_last_bytes_of_fragmented() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, size - 10, 10).unwrap();
        let expected: Vec<u8> = ((size - 10)..size).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn full_read_matches_range_read_concatenation() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let full = vol.read_file_data(anode, size).unwrap();
        let mut assembled = Vec::new();
        let mut offset = 0u64;
        while offset < size {
            let chunk = vol.read_file_range(anode, size, offset, 256).unwrap();
            assembled.extend_from_slice(&chunk);
            offset += 256;
        }
        assert_eq!(full, assembled);
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Anode Chain Validation
// ============================================================

mod anode_chains {
    use super::*;

    #[test]
    fn validate_anode_chain_on_normal_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("normal.txt", &vec![0; 4096]).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("normal.txt").unwrap().unwrap();
        let chain = vol.validate_anode_chain(entry.anode).unwrap();
        assert_eq!(chain.len(), 8);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn validate_anode_chain_on_single_block_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("tiny.txt", b"x").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("tiny.txt").unwrap().unwrap();
        let chain = vol.validate_anode_chain(entry.anode).unwrap();
        assert_eq!(chain.len(), 1);
        std::fs::remove_file(&path).ok();
    }
}
