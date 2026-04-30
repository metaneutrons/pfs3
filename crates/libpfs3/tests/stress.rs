// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Stress/capacity tests: many files, deep nesting, fill+empty, create/delete
//! cycles, fragmentation, reserved exhaustion, large files, dir block overflow.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(14000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_stress_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "Stress".into(),
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
fn many_files_in_root() {
    let path = fresh_image(32768);
    let mut w = open_writer(&path);
    for i in 0..100 {
        w.write_file(
            &format!("file_{:04}.txt", i),
            format!("data {}", i).as_bytes(),
        )
        .unwrap();
    }
    drop(w);
    let mut vol = reopen(&path);
    let entries = vol.list_dir("/").unwrap();
    assert_eq!(entries.len(), 100);
    assert_eq!(vol.read_file("file_0000.txt").unwrap(), b"data 0");
    assert_eq!(vol.read_file("file_0099.txt").unwrap(), b"data 99");
    std::fs::remove_file(&path).ok();
}

#[test]
fn many_files_in_subdirectory() {
    let path = fresh_image(32768);
    let mut w = open_writer(&path);
    w.create_dir("Bulk").unwrap();
    for i in 0..50 {
        w.write_file(
            &format!("Bulk/item_{:03}.dat", i),
            &vec![(i & 0xFF) as u8; 256],
        )
        .unwrap();
    }
    drop(w);
    let mut vol = reopen(&path);
    assert_eq!(vol.list_dir("Bulk").unwrap().len(), 50);
    std::fs::remove_file(&path).ok();
}

#[test]
fn deep_directory_nesting() {
    let path = fresh_image(8192);
    let mut w = open_writer(&path);
    let mut current = String::new();
    for i in 0..20 {
        current = if current.is_empty() {
            format!("d{}", i)
        } else {
            format!("{}/d{}", current, i)
        };
        w.create_dir(&current).unwrap();
    }
    let deepfile = format!("{}/deep.txt", current);
    w.write_file(&deepfile, b"bottom").unwrap();
    drop(w);
    let mut vol = reopen(&path);
    assert_eq!(vol.read_file(&deepfile).unwrap(), b"bottom");
    std::fs::remove_file(&path).ok();
}

#[test]
fn repeated_create_delete_cycles() {
    let path = fresh_image(4096);
    let mut w = open_writer(&path);
    let initial_free = w.vol.free_blocks();
    for cycle in 0..20 {
        let name = format!("cycle_{}.txt", cycle);
        w.write_file(&name, &vec![cycle as u8; 512]).unwrap();
        w.delete(&name).unwrap();
    }
    let final_free = w.vol.free_blocks();
    assert!(
        final_free >= initial_free - 2,
        "leaked blocks: initial={}, final={}",
        initial_free,
        final_free
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn repeated_overwrite_same_file() {
    let path = fresh_image(4096);
    for i in 0u32..30 {
        let size = ((i * 137) % 2048 + 1) as usize;
        let data = vec![(i & 0xFF) as u8; size];
        let mut w = open_writer(&path);
        w.write_file("rewrite.bin", &data).unwrap();
        drop(w);
    }
    let size = ((29u32 * 137) % 2048 + 1) as usize;
    let expected = vec![29u8; size];
    let mut vol = reopen(&path);
    assert_eq!(vol.read_file("rewrite.bin").unwrap(), expected);
    std::fs::remove_file(&path).ok();
}

#[test]
fn interleaved_create_and_delete() {
    let path = fresh_image(8192);
    let mut w = open_writer(&path);
    for i in 0..20 {
        w.write_file(&format!("f{}.txt", i), &vec![i as u8; 128])
            .unwrap();
    }
    for i in (0..20).step_by(2) {
        w.delete(&format!("f{}.txt", i)).unwrap();
    }
    for i in 0..10 {
        w.write_file(&format!("new{}.txt", i), &vec![(i + 100) as u8; 256])
            .unwrap();
    }
    drop(w);
    let mut vol = reopen(&path);
    let entries = vol.list_dir("/").unwrap();
    assert_eq!(entries.len(), 20);
    for i in (1..20).step_by(2) {
        assert_eq!(
            vol.read_file(&format!("f{}.txt", i)).unwrap(),
            vec![i as u8; 128]
        );
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn large_file_multi_extent() {
    let path = fresh_image(512);
    let mut w = open_writer(&path);
    for i in 0..5 {
        w.write_file(&format!("frag{}.txt", i), &vec![i as u8; 512])
            .unwrap();
    }
    w.delete("frag1.txt").unwrap();
    w.delete("frag3.txt").unwrap();
    let big: Vec<u8> = (0..8192).map(|i| (i % 256) as u8).collect();
    w.write_file("big.bin", &big).unwrap();
    drop(w);
    let mut vol = reopen(&path);
    assert_eq!(vol.read_file("big.bin").unwrap(), big);
    std::fs::remove_file(&path).ok();
}

#[test]
fn fill_and_empty_disk() {
    let path = fresh_image(2048);
    let mut w = open_writer(&path);
    let initial_free = w.vol.free_blocks();
    let mut written = Vec::new();
    for i in 0..500 {
        let name = format!("fill_{}.txt", i);
        if w.write_file(&name, &vec![0u8; 512]).is_ok() {
            written.push(name);
        } else {
            break;
        }
    }
    assert!(!written.is_empty());
    let free_after_fill = w.vol.free_blocks();
    assert!(
        free_after_fill < initial_free,
        "should have used some blocks: initial={}, after={}",
        initial_free,
        free_after_fill
    );
    for name in &written {
        w.delete(name).unwrap();
    }
    let final_free = w.vol.free_blocks();
    assert!(
        final_free >= initial_free - 2,
        "leaked: initial={}, final={}",
        initial_free,
        final_free
    );
    std::fs::remove_file(&path).ok();
}

#[test]
fn dir_block_overflow_many_entries() {
    let path = fresh_image(8192);
    let mut w = open_writer(&path);
    let count = 45;
    for i in 0..count {
        w.write_file(&format!("f{:03}.txt", i), &[i as u8; 4])
            .unwrap();
    }
    drop(w);
    let mut vol = reopen(&path);
    let entries = vol.list_dir("/").unwrap();
    assert_eq!(entries.len(), count);
    for i in 0..count {
        let data = vol.read_file(&format!("f{:03}.txt", i)).unwrap();
        assert_eq!(data, vec![i as u8; 4]);
    }
    std::fs::remove_file(&path).ok();
}

#[test]
fn multiple_dir_blocks_with_delete() {
    let path = fresh_image(8192);
    let mut w = open_writer(&path);
    for i in 0..50 {
        w.write_file(&format!("item{:03}.dat", i), &[i as u8; 8])
            .unwrap();
    }
    for i in (10..30).rev() {
        w.delete(&format!("item{:03}.dat", i)).unwrap();
    }
    drop(w);
    let mut vol = reopen(&path);
    let entries = vol.list_dir("/").unwrap();
    assert_eq!(entries.len(), 30);
    std::fs::remove_file(&path).ok();
}

#[test]
fn large_file_uses_multiple_extents() {
    let path = fresh_image(4096);
    let mut w = open_writer(&path);
    let data: Vec<u8> = (0..102400).map(|i| (i % 256) as u8).collect();
    w.write_file("large.bin", &data).unwrap();
    drop(w);
    let mut vol = reopen(&path);
    let entry = vol.lookup("large.bin").unwrap().unwrap();
    assert_eq!(entry.file_size(), 102400);
    let chain = vol.validate_anode_chain(entry.anode).unwrap();
    assert!(chain.len() >= 200);
    let read_data = vol.read_file("large.bin").unwrap();
    assert_eq!(read_data, data);
    std::fs::remove_file(&path).ok();
}

// ============================================================
// Reserved Block Exhaustion
// ============================================================

#[test]
fn many_directories_exhaust_reserved_gracefully() {
    let path = fresh_image(4096);
    let mut w = open_writer(&path);
    let mut created = 0;
    for i in 0..200 {
        if w.create_dir(&format!("dir{:04}", i)).is_ok() {
            created += 1;
        } else {
            break;
        }
    }
    assert!(created > 10, "should create at least some dirs");
    drop(w);
    let mut vol = reopen(&path);
    let entries = vol.list_dir("/").unwrap();
    assert_eq!(entries.len(), created);
    std::fs::remove_file(&path).ok();
}

#[test]
fn reserved_exhaustion_returns_disk_full() {
    let path = fresh_image(256);
    let mut w = open_writer(&path);
    let mut last_err = None;
    for i in 0..100 {
        match w.create_dir(&format!("d{}", i)) {
            Ok(_) => {}
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }
    if let Some(e) = last_err {
        let msg = format!("{}", e);
        assert!(
            msg.contains("full") || msg.contains("not enough") || msg.contains("no free"),
            "unexpected error: {}",
            msg
        );
    }
    std::fs::remove_file(&path).ok();
}
