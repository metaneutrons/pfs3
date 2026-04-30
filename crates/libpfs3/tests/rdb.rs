// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! RDB partition detection and corruption tests.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::rdb;

static COUNTER: AtomicU32 = AtomicU32::new(16000);

fn temp_path() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("pfs3_rdb_{}.img", n))
}

fn put_be32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}

#[test]
fn no_rdb_signature_returns_empty() {
    let path = temp_path();
    std::fs::write(&path, &vec![0u8; 32768]).unwrap();
    let parts = rdb::detect_pfs3_partitions(&path).unwrap();
    assert!(parts.is_empty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn rdb_with_no_partitions_returns_empty() {
    let path = temp_path();
    let mut data = vec![0u8; 32768];
    data[0..4].copy_from_slice(b"RDSK");
    put_be32(&mut data, 4, 64);
    put_be32(&mut data, 0x0C, 7);
    std::fs::write(&path, &data).unwrap();
    let parts = rdb::detect_pfs3_partitions(&path).unwrap();
    assert!(parts.is_empty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn rdb_with_non_pfs3_partition_skipped() {
    let path = temp_path();
    let mut data = vec![0u8; 65536];
    data[0..4].copy_from_slice(b"RDSK");
    put_be32(&mut data, 4, 64);
    put_be32(&mut data, 0x0C, 7);
    let part_off = 512;
    data[part_off..part_off + 4].copy_from_slice(b"PART");
    put_be32(&mut data, part_off + 4, 64);
    data[part_off + 0x24] = 3;
    data[part_off + 0x25..part_off + 0x28].copy_from_slice(b"DH0");
    let env = part_off + 0x80;
    put_be32(&mut data, env + 0x0C, 2);
    put_be32(&mut data, env + 0x14, 32);
    put_be32(&mut data, env + 0x24, 2);
    put_be32(&mut data, env + 0x28, 9);
    put_be32(&mut data, env + 0x40, 0x444F5301);
    std::fs::write(&path, &data).unwrap();
    let parts = rdb::detect_pfs3_partitions(&path).unwrap();
    assert!(parts.is_empty());
    std::fs::remove_file(&path).ok();
}

#[test]
fn rdb_truncated_file_no_panic() {
    let path = temp_path();
    let mut data = vec![0u8; 512];
    data[0..4].copy_from_slice(b"RDSK");
    put_be32(&mut data, 4, 64);
    put_be32(&mut data, 0x0C, 63);
    std::fs::write(&path, &data).unwrap();
    let _ = rdb::detect_pfs3_partitions(&path);
    std::fs::remove_file(&path).ok();
}

#[test]
fn rdb_highblock_zero_no_panic() {
    let path = temp_path();
    let mut data = vec![0u8; 32768];
    data[0..4].copy_from_slice(b"RDSK");
    put_be32(&mut data, 4, 64);
    put_be32(&mut data, 0x0C, 0);
    std::fs::write(&path, &data).unwrap();
    let parts = rdb::detect_pfs3_partitions(&path).unwrap();
    assert!(parts.is_empty());
    std::fs::remove_file(&path).ok();
}
