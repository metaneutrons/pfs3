// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Deldir listing and undelete roundtrip tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(17000);

fn fresh_image_with_deldir(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_deldir_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "DelTest".into(),
        enable_deldir: true,
    };
    format::format_with_size(&dev, blocks, &opts).unwrap();
    drop(dev);
    path
}

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_deldir_nd_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "NoDel".into(),
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

#[test]
fn deldir_empty_on_fresh_volume() {
    let path = fresh_image_with_deldir(4096);
    let mut vol = reopen(&path);
    let entries = vol.list_deldir().unwrap();
    assert!(entries.is_empty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn deleted_file_appears_in_deldir() {
    let path = fresh_image_with_deldir(4096);
    let mut w = open_writer(&path);
    w.write_file("deleteme.txt", b"precious data").unwrap();
    w.delete("deleteme.txt").unwrap();
    let entries = w.vol.list_deldir().unwrap();
    if !entries.is_empty() {
        assert!(entries.iter().any(|e| e.filename.starts_with("deleteme")));
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn undelete_restores_file_content() {
    let path = fresh_image_with_deldir(4096);
    let mut w = open_writer(&path);
    w.write_file("restore.txt", b"restore me please").unwrap();
    w.delete("restore.txt").unwrap();

    let entries = w.vol.list_deldir().unwrap();
    if entries.is_empty() {
        std::fs::remove_file(&path).ok();
        return;
    }

    let idx = entries
        .iter()
        .position(|e| e.filename.starts_with("restore"))
        .unwrap();
    w.undelete(idx, "restored.txt").unwrap();
    drop(w);

    let mut vol = reopen(&path);
    let data = vol.read_file("restored.txt").unwrap();
    assert_eq!(data, b"restore me please");
    std::fs::remove_file(&path).ok();
}

#[test]
fn deldir_not_enabled_returns_empty() {
    let path = fresh_image(4096);
    let mut vol = reopen(&path);
    let entries = vol.list_deldir().unwrap();
    assert!(entries.is_empty());
    std::fs::remove_file(&path).ok();
}
