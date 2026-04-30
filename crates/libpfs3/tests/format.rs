// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Format tests: mkfs various sizes, minimum disk, rootblock flags, format_check_passes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;

static COUNTER: AtomicU32 = AtomicU32::new(12000);

fn temp_image(size_blocks: u64) -> (PathBuf, FileBlockDevice) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_test_mkfs_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, size_blocks).unwrap();
    (path, dev)
}

fn reopen(path: &Path) -> Volume {
    Volume::open(path, 0).unwrap()
}

#[test]
fn format_and_open() {
    let (path, dev) = temp_image(8192);
    let opts = FormatOptions {
        volume_name: "TestFmt".into(),
        enable_deldir: false,
    };
    let result = format::format_with_size(&dev, 8192, &opts).unwrap();
    assert_eq!(result.volume_name, "TestFmt");
    assert_eq!(result.total_blocks, 8192);
    assert!(result.data_blocks > 0);
    drop(dev);

    let vol = Volume::open(&path, 0).unwrap();
    assert_eq!(vol.name(), "TestFmt");
    assert_eq!(vol.rootblock.disktype, ID_PFS_DISK);
    assert_eq!(vol.total_blocks(), 8192);
    std::fs::remove_file(&path).ok();
}

#[test]
fn format_empty_root() {
    let (path, dev) = temp_image(4096);
    let opts = FormatOptions {
        volume_name: "Empty".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, 4096, &opts).unwrap();
    drop(dev);

    let mut vol = Volume::open(&path, 0).unwrap();
    let entries = vol.list_dir("/").unwrap();
    assert!(entries.is_empty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn format_rootblock_flags() {
    let (path, dev) = temp_image(8192);
    let opts = FormatOptions {
        volume_name: "Flags".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, 8192, &opts).unwrap();
    drop(dev);

    let vol = Volume::open(&path, 0).unwrap();
    assert!(vol.rootblock.has_flag(MODE_HARDDISK));
    assert!(vol.rootblock.has_flag(MODE_SPLITTED_ANODES));
    assert!(vol.rootblock.has_flag(MODE_EXTENSION));
    assert!(vol.rootblock.has_flag(MODE_LONGFN));
    assert!(vol.rootblock.has_flag(MODE_DATESTAMP));
    assert!(vol.rootblock_ext.is_some());
    std::fs::remove_file(&path).ok();
}

#[test]
fn format_check_passes() {
    let (path, dev) = temp_image(8192);
    let opts = FormatOptions {
        volume_name: "Check".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, 8192, &opts).unwrap();
    drop(dev);

    let mut vol = Volume::open(&path, 0).unwrap();
    let blocks = vol.validate_anode_chain(ANODE_ROOTDIR).unwrap();
    assert!(!blocks.is_empty());
    assert!(vol.free_blocks() > 0);
    assert!(vol.free_blocks() < vol.total_blocks());
    std::fs::remove_file(&path).ok();
}

#[test]
fn format_various_sizes() {
    for &blocks in &[256u64, 1024, 4096, 16384] {
        let (path, dev) = temp_image(blocks);
        let opts = FormatOptions {
            volume_name: "Size".into(),
            enable_deldir: false,
        };
        let result = format::format_with_size(&dev, blocks, &opts).unwrap();
        assert_eq!(result.total_blocks, blocks);
        assert!(result.data_blocks > 0);
        drop(dev);

        let vol = Volume::open(&path, 0).unwrap();
        assert_eq!(vol.total_blocks() as u64, blocks);
        std::fs::remove_file(&path).ok();
    }
}

#[test]
fn format_minimum_size() {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_min_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, 64).unwrap();
    let opts = FormatOptions {
        volume_name: "Tiny".into(),
        enable_deldir: false,
    };
    assert!(format::format_with_size(&dev, 64, &opts).is_err());
    drop(dev);
    let dev = FileBlockDevice::create(&path, 512, 128).unwrap();
    format::format_with_size(&dev, 128, &opts).unwrap();
    drop(dev);
    let vol = reopen(&path);
    assert_eq!(vol.name(), "Tiny");
    assert!(vol.total_blocks() > 0);
    std::fs::remove_file(&path).ok();
}

#[test]
fn format_then_write_on_minimum_disk() {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_minw_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, 128).unwrap();
    let opts = FormatOptions {
        volume_name: "MinW".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, 128, &opts).unwrap();
    drop(dev);
    let dev = FileBlockDevice::open_rw(&path, 512, 0, 0).unwrap();
    let vol = Volume::from_device(Box::new(dev)).unwrap();
    let mut w = libpfs3::writer::Writer::open(vol).unwrap();
    w.write_file("tiny.txt", b"hi").unwrap();
    drop(w);
    let mut vol = reopen(&path);
    assert_eq!(vol.read_file("tiny.txt").unwrap(), b"hi");
    std::fs::remove_file(&path).ok();
}
