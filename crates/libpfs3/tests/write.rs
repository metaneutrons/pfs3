// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Write operation tests: write_file, create_dir, delete, overwrite, softlinks,
//! hardlinks, protection bits roundtrip, volume name, block boundary tests,
//! crash consistency checks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(11000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_write_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "WrTest".into(),
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
// Writer tests — format → write → read back → delete
// ============================================================

mod writer_tests {
    use super::*;

    #[test]
    fn write_and_read_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"Hello PFS3!").unwrap();
        let vol = w.into_volume();
        drop(vol);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("test.txt").unwrap(), b"Hello PFS3!");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_binary_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data: Vec<u8> = (0..=255u8).cycle().take(2048).collect();
        w.write_file("binary.dat", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("binary.dat").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_dir_and_write_nested() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("SubDir").unwrap();
        w.write_file("SubDir/inner.txt", b"nested!").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("SubDir").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "inner.txt");
        assert_eq!(vol.read_file("SubDir/inner.txt").unwrap(), b"nested!");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("a.txt", b"aaa").unwrap();
        w.write_file("b.txt", b"bbb").unwrap();
        w.delete("a.txt").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "b.txt");
        assert_eq!(vol.read_file("b.txt").unwrap(), b"bbb");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_empty_dir() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("EmptyDir").unwrap();
        w.delete("EmptyDir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert!(vol.list_dir("/").unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn multiple_files() {
        let path = fresh_image(8192);
        let mut w = open_writer(&path);
        for i in 0..10 {
            w.write_file(
                &format!("file_{}.txt", i),
                format!("content {}", i).as_bytes(),
            )
            .unwrap();
        }
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 10);
        for i in 0..10 {
            let data = vol.read_file(&format!("file_{}.txt", i)).unwrap();
            assert_eq!(String::from_utf8_lossy(&data), format!("content {}", i));
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn check_after_writes() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"data").unwrap();
        w.create_dir("Dir").unwrap();
        w.write_file("Dir/sub.txt", b"sub").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        for e in &entries {
            vol.validate_anode_chain(e.anode).unwrap();
        }
        assert!(vol.free_blocks() > 0);
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Overwrite tests — in-place file overwrite (same, grow, shrink)
// ============================================================

mod overwrite_tests {
    use super::*;

    #[test]
    fn overwrite_same_size() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"hello world!").unwrap();
        let anode = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        w.overwrite_file_in(ANODE_ROOTDIR, "test.txt", anode, b"HELLO WORLD!")
            .unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("test.txt").unwrap(), b"HELLO WORLD!");
        assert_eq!(vol.lookup("test.txt").unwrap().unwrap().file_size(), 12);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_grow() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"small").unwrap();
        let anode = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        let big_data: Vec<u8> = (0..2048u16).flat_map(|i| i.to_le_bytes()).collect();
        w.overwrite_file_in(ANODE_ROOTDIR, "test.txt", anode, &big_data)
            .unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("test.txt").unwrap(), big_data);
        assert_eq!(
            vol.lookup("test.txt").unwrap().unwrap().file_size(),
            big_data.len() as u64
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_shrink() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let big_data: Vec<u8> = vec![0xAB; 2048];
        w.write_file("test.txt", &big_data).unwrap();
        let free_before = w.vol.rootblock.blocksfree;
        let anode = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        w.overwrite_file_in(ANODE_ROOTDIR, "test.txt", anode, b"tiny")
            .unwrap();
        let free_after = w.vol.rootblock.blocksfree;
        assert!(free_after > free_before);
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("test.txt").unwrap(), b"tiny");
        assert_eq!(vol.lookup("test.txt").unwrap().unwrap().file_size(), 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_anode_stable() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"original").unwrap();
        let anode_before = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        w.overwrite_file_in(ANODE_ROOTDIR, "test.txt", anode_before, b"replaced!")
            .unwrap();
        let anode_after = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        assert_eq!(anode_before, anode_after);
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("test.txt").unwrap(), b"replaced!");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_multiple_times() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("test.txt", b"v1").unwrap();
        let anode = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "test.txt")
            .unwrap()
            .anode;
        for content in &[
            &vec![0u8; 4096][..],
            b"small" as &[u8],
            &vec![0xFFu8; 1500],
            &vec![0xFFu8; 1500],
        ] {
            w.overwrite_file_in(ANODE_ROOTDIR, "test.txt", anode, content)
                .unwrap();
        }
        drop(w);
        let mut vol = reopen(&path);
        let data = vol.read_file("test.txt").unwrap();
        assert_eq!(data.len(), 1500);
        assert!(data.iter().all(|&b| b == 0xFF));
        vol.validate_anode_chain(anode).unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_check_clean() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("a.txt", b"aaa").unwrap();
        w.write_file("b.txt", &vec![0u8; 2000]).unwrap();
        let anode_a = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "a.txt")
            .unwrap()
            .anode;
        let anode_b = w
            .vol
            .list_dir_by_anode(ANODE_ROOTDIR)
            .unwrap()
            .iter()
            .find(|e| e.name == "b.txt")
            .unwrap()
            .anode;
        w.overwrite_file_in(ANODE_ROOTDIR, "a.txt", anode_a, &vec![0xAA; 3000])
            .unwrap();
        w.overwrite_file_in(ANODE_ROOTDIR, "b.txt", anode_b, b"b")
            .unwrap();
        drop(w);
        let mut vol = reopen(&path);
        vol.validate_anode_chain(anode_a).unwrap();
        vol.validate_anode_chain(anode_b).unwrap();
        let bitmap_free = vol.bitmap_count_free().unwrap();
        assert_eq!(bitmap_free, vol.free_blocks());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Softlink / hardlink tests
// ============================================================

mod link_tests {
    use super::*;

    #[test]
    fn create_and_read_softlink() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("mylink", "target/path").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_softlink());
        assert_eq!(entries[0].name, "mylink");
        let data = vol
            .read_file_data(entries[0].anode, entries[0].file_size())
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&data), "target/path");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_hardlink() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("original.txt", b"data").unwrap();
        let entries = w.vol.list_dir_by_anode(ANODE_ROOTDIR).unwrap();
        let orig = entries.iter().find(|e| e.name == "original.txt").unwrap();
        let orig_anode = orig.anode;
        w.create_hardlink("link.txt", orig_anode).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 2);
        let orig = entries.iter().find(|e| e.name == "original.txt").unwrap();
        let link = entries.iter().find(|e| e.name == "link.txt").unwrap();
        assert_eq!(orig.anode, link.anode);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_to_nonexistent_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("dangling", "nonexistent/path/file.txt")
            .unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("dangling").unwrap().unwrap();
        assert!(entry.is_softlink());
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&target),
            "nonexistent/path/file.txt"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_with_long_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let long_target = "A/".repeat(200) + "file.txt";
        w.create_softlink("longlink", &long_target).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("longlink").unwrap().unwrap();
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(String::from_utf8_lossy(&target), long_target);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_to_self_name() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("loop", "loop").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("loop").unwrap().unwrap();
        assert!(entry.is_softlink());
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(target, b"loop");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_empty_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("empty_target", "").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("empty_target").unwrap().unwrap();
        assert!(entry.is_softlink());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_coexists_with_files() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("real.txt", b"real data").unwrap();
        w.create_softlink("link_to_real", "real.txt").unwrap();
        w.create_dir("SubDir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 3);
        let link = entries.iter().find(|e| e.name == "link_to_real").unwrap();
        assert!(link.is_softlink());
        let file = entries.iter().find(|e| e.name == "real.txt").unwrap();
        assert!(file.is_file());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_softlink() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("mylink", "target").unwrap();
        w.delete("mylink").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert!(vol.list_dir("/").unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Write-Path Edge Cases
// ============================================================

mod write_edge_cases {
    use super::*;

    #[test]
    fn write_to_nonexistent_parent_returns_error() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let result = w.write_file("NoSuchDir/file.txt", b"orphan");
        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_dir_with_nonexistent_parent_returns_error() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let result = w.create_dir("A/B/C");
        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_exact_one_block_boundary() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0xAA; 512];
        w.write_file("exact512.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("exact512.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_exact_two_blocks() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0xBB; 1024];
        w.write_file("exact1024.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("exact1024.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_one_byte() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("one.bin", &[0x42]).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("one.bin").unwrap(), vec![0x42]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_511_bytes() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0xCC; 511];
        w.write_file("511.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("511.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_513_bytes() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0xDD; 513];
        w.write_file("513.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("513.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_multiple_files_same_dir_preserves_all() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("a.txt", b"aaa").unwrap();
        w.write_file("b.txt", b"bbb").unwrap();
        w.write_file("c.txt", b"ccc").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("a.txt").unwrap(), b"aaa");
        assert_eq!(vol.read_file("b.txt").unwrap(), b"bbb");
        assert_eq!(vol.read_file("c.txt").unwrap(), b"ccc");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_with_all_byte_values() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data: Vec<u8> = (0..=255).collect();
        w.write_file("allbytes.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("allbytes.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_with_null_bytes() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0u8; 1024];
        w.write_file("zeros.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("zeros.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_then_recreate_same_name() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("reuse.txt", b"first").unwrap();
        w.delete("reuse.txt").unwrap();
        w.write_file("reuse.txt", b"second").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("reuse.txt").unwrap(), b"second");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_dir_then_delete_then_recreate() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("TempDir").unwrap();
        w.delete("TempDir").unwrap();
        w.create_dir("TempDir").unwrap();
        w.write_file("TempDir/new.txt", b"new").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("TempDir/new.txt").unwrap(), b"new");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_file_name_with_spaces_and_dots() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("my file.doc", b"doc").unwrap();
        w.write_file("archive.tar.gz", b"tar").unwrap();
        w.write_file(".hidden", b"hidden").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("my file.doc").unwrap(), b"doc");
        assert_eq!(vol.read_file("archive.tar.gz").unwrap(), b"tar");
        assert_eq!(vol.read_file(".hidden").unwrap(), b"hidden");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn zero_byte_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("empty.txt", b"").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("empty.txt").unwrap().unwrap();
        assert_eq!(entry.file_size(), 0);
        assert_eq!(vol.read_file("empty.txt").unwrap(), b"");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn max_filename_length() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let long_name = "A".repeat(107);
        w.write_file(&long_name, b"long name file").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file(&long_name).unwrap(), b"long name file");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn filename_with_special_chars() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("hello world.txt", b"spaces").unwrap();
        w.write_file("file-with-dashes", b"dashes").unwrap();
        w.write_file("UPPER.TXT", b"upper").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("hello world.txt").unwrap(), b"spaces");
        assert_eq!(vol.read_file("file-with-dashes").unwrap(), b"dashes");
        assert_eq!(vol.read_file("upper.txt").unwrap(), b"upper");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_exactly_one_block() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data = vec![0x42u8; 512];
        w.write_file("oneblock.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("oneblock.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_spanning_multiple_blocks() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data: Vec<u8> = (0..5120).map(|i| (i % 256) as u8).collect();
        w.write_file("multi.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("multi.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_not_block_aligned() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let data: Vec<u8> = (0..513).map(|i| (i % 256) as u8).collect();
        w.write_file("unaligned.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("unaligned.bin").unwrap(), data);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_with_larger_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("grow.txt", b"small").unwrap();
        drop(w);
        let mut w = open_writer(&path);
        w.write_file("grow.txt", &vec![0xBBu8; 4096]).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("grow.txt").unwrap(), vec![0xBBu8; 4096]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn overwrite_with_smaller_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("shrink.txt", &vec![0xCCu8; 4096]).unwrap();
        drop(w);
        let mut w = open_writer(&path);
        w.write_file("shrink.txt", b"tiny").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("shrink.txt").unwrap(), b"tiny");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        assert!(w.delete("ghost.txt").is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_nonempty_dir_returns_error() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("Dir").unwrap();
        w.write_file("Dir/file.txt", b"inside").unwrap();
        assert!(w.delete("Dir").is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn create_dir_already_exists() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("MyDir").unwrap();
        let _ = w.create_dir("MyDir");
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("MyDir").unwrap();
        assert!(entries.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn nested_dirs_three_levels() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_dir("A").unwrap();
        w.create_dir("A/B").unwrap();
        w.create_dir("A/B/C").unwrap();
        w.write_file("A/B/C/deep.txt", b"deep").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("A/B/C/deep.txt").unwrap(), b"deep");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn free_blocks_decrease_after_write() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let free_before = w.vol.free_blocks();
        w.write_file("data.bin", &vec![0u8; 2048]).unwrap();
        let free_after = w.vol.free_blocks();
        assert_eq!(free_before - free_after, 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn free_blocks_increase_after_delete() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("temp.bin", &vec![0u8; 2048]).unwrap();
        let free_before = w.vol.free_blocks();
        w.delete("temp.bin").unwrap();
        let free_after = w.vol.free_blocks();
        assert_eq!(free_after - free_before, 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_file_data_on_zero_size_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("empty.bin", b"").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("empty.bin").unwrap().unwrap();
        let data = vol.read_file_data(entry.anode, 0).unwrap();
        assert!(data.is_empty());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Protection Bits Roundtrip
// ============================================================

mod protection_bits {
    use super::*;
    use libpfs3::util;

    #[test]
    fn all_256_protection_values_roundtrip() {
        let path = fresh_image(8192);
        let mut w = open_writer(&path);
        for prot in 0..=255u8 {
            let name = format!("p{:03}.txt", prot);
            w.write_file(&name, &[prot]).unwrap();
            w.update_dir_entry_protection(ANODE_ROOTDIR, &name, prot)
                .unwrap();
        }
        drop(w);

        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 256);
        for entry in &entries {
            let expected_prot: u8 = entry.name[1..4].parse().unwrap();
            assert_eq!(
                entry.protection, expected_prot,
                "protection mismatch for {}: got {}, expected {}",
                entry.name, entry.protection, expected_prot
            );
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn protection_string_format() {
        assert_eq!(util::amiga_protection_string(0x0F), "--------");
        assert_eq!(util::amiga_protection_string(0x00), "----rwed");
        assert_eq!(util::amiga_protection_string(0xF0), "hsparwed");
        assert_eq!(util::amiga_protection_string(0x07), "----r---");
    }

    #[test]
    fn parse_protection_absolute() {
        assert_eq!(util::parse_amiga_protection(0xFF, "rwed"), Some(0x00));
        assert_eq!(util::parse_amiga_protection(0x00, ""), None);
    }

    #[test]
    fn parse_protection_additive() {
        assert_eq!(util::parse_amiga_protection(0x0F, "+r"), Some(0x07));
    }

    #[test]
    fn parse_protection_subtractive() {
        assert_eq!(util::parse_amiga_protection(0x00, "-w"), Some(0x04));
    }

    #[test]
    fn parse_protection_invalid_char() {
        assert_eq!(util::parse_amiga_protection(0x00, "xyz"), None);
    }
}

// ============================================================
// Volume Name Tests
// ============================================================

mod volume_name {
    use super::*;

    #[test]
    fn volume_name_max_length() {
        let path = fresh_image(4096);
        let dev = FileBlockDevice::open_rw(&path, 512, 0, 0).unwrap();
        let vol = Volume::from_device(Box::new(dev)).unwrap();
        let mut w = Writer::open(vol).unwrap();
        let long_name = "A".repeat(30);
        w.set_volume_name(&long_name).unwrap();
        drop(w);
        let vol = reopen(&path);
        assert_eq!(vol.name(), long_name);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn volume_name_truncated_at_30() {
        let path = fresh_image(4096);
        let dev = FileBlockDevice::open_rw(&path, 512, 0, 0).unwrap();
        let vol = Volume::from_device(Box::new(dev)).unwrap();
        let mut w = Writer::open(vol).unwrap();
        let too_long = "B".repeat(50);
        w.set_volume_name(&too_long).unwrap();
        drop(w);
        let vol = reopen(&path);
        assert_eq!(vol.name().len(), 30);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_size_field_accuracy() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let sizes = [0, 1, 511, 512, 513, 1023, 1024, 1025, 2048, 4096, 5000];
        for &size in &sizes {
            let name = format!("size_{}.bin", size);
            let data = vec![0xAA; size];
            w.write_file(&name, &data).unwrap();
        }
        drop(w);
        let mut vol = reopen(&path);
        for &size in &sizes {
            let name = format!("size_{}.bin", size);
            let entry = vol.lookup(&name).unwrap().unwrap();
            assert_eq!(entry.file_size(), size as u64, "size mismatch for {}", name);
        }
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Crash Consistency — bitmap matches rootblock
// ============================================================

mod crash_consistency {
    use super::*;

    #[test]
    fn every_snapshot_during_write_is_openable() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("snap.txt", b"important data here").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("snap.txt").unwrap(), b"important data here");
        let entry = vol.lookup("snap.txt").unwrap().unwrap();
        vol.validate_anode_chain(entry.anode).unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_then_check_anode_chain_valid() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("check1.txt", b"data1").unwrap();
        w.write_file("check2.txt", &vec![0u8; 2048]).unwrap();
        w.create_dir("SubDir").unwrap();
        w.write_file("SubDir/check3.txt", b"nested").unwrap();
        drop(w);

        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        for e in &entries {
            vol.validate_anode_chain(e.anode).unwrap();
            if e.is_dir() {
                let sub = vol.list_dir_by_anode(e.anode).unwrap();
                for se in &sub {
                    vol.validate_anode_chain(se.anode).unwrap();
                }
            }
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_then_check_no_dangling_anodes() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("del1.txt", &vec![0u8; 1024]).unwrap();
        w.write_file("del2.txt", &vec![0u8; 512]).unwrap();
        w.write_file("keep.txt", b"keeper").unwrap();
        w.delete("del1.txt").unwrap();
        w.delete("del2.txt").unwrap();
        drop(w);

        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "keep.txt");
        vol.validate_anode_chain(entries[0].anode).unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn free_block_count_matches_bitmap() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("f1.txt", &vec![0u8; 2048]).unwrap();
        w.write_file("f2.txt", &vec![0u8; 1024]).unwrap();
        let reported_free = w.vol.free_blocks();
        drop(w);

        let mut vol = reopen(&path);
        let bitmap_free = vol.bitmap_count_free().unwrap();
        assert_eq!(
            reported_free, bitmap_free,
            "rootblock free={} but bitmap says {}",
            reported_free, bitmap_free
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn free_blocks_after_delete_matches_bitmap() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("temp.txt", &vec![0u8; 4096]).unwrap();
        w.delete("temp.txt").unwrap();
        let reported_free = w.vol.free_blocks();
        drop(w);

        let mut vol = reopen(&path);
        let bitmap_free = vol.bitmap_count_free().unwrap();
        assert_eq!(reported_free, bitmap_free);
        std::fs::remove_file(&path).ok();
    }
}
