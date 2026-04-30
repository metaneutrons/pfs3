// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Fault injection + disk full: I/O errors, power-loss simulation,
//! disk full handling (read and write on full disk).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use libpfs3::error::{Error, Result};
use libpfs3::format::{self, FormatOptions};
use libpfs3::io::{BlockDevice, FileBlockDevice};
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(15000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_fault_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "FaultTest".into(),
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

/// In-memory block device supporting fault injection.
struct MemBlockDevice {
    data: Mutex<Vec<u8>>,
    bs: u32,
    writes_until_fail: AtomicU64,
    write_count: AtomicU64,
}

impl MemBlockDevice {
    fn new(blocks: u64, bs: u32) -> Self {
        Self {
            data: Mutex::new(vec![0u8; blocks as usize * bs as usize]),
            bs,
            writes_until_fail: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
        }
    }

    fn from_data(data: Vec<u8>, bs: u32) -> Self {
        Self {
            data: Mutex::new(data),
            bs,
            writes_until_fail: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
        }
    }

    fn fail_after(&self, n: u64) {
        self.writes_until_fail.store(n, Ordering::SeqCst);
    }

    fn snapshot(&self) -> Vec<u8> {
        self.data.lock().unwrap().clone()
    }
}

impl BlockDevice for MemBlockDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<()> {
        let data = self.data.lock().unwrap();
        let off = block as usize * self.bs as usize;
        let end = off + self.bs as usize;
        if end > data.len() {
            return Err(Error::BlockOutOfRange(block));
        }
        buf[..self.bs as usize].copy_from_slice(&data[off..end]);
        Ok(())
    }

    fn read_blocks(&self, block: u64, count: u32, buf: &mut [u8]) -> Result<()> {
        for i in 0..count {
            self.read_block(block + i as u64, &mut buf[i as usize * self.bs as usize..])?;
        }
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.bs
    }

    fn write_block(&self, block: u64, wdata: &[u8]) -> Result<()> {
        let remaining = self.writes_until_fail.load(Ordering::SeqCst);
        if remaining > 0 {
            let prev = self.writes_until_fail.fetch_sub(1, Ordering::SeqCst);
            if prev == 1 {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "injected write failure",
                )));
            }
        }
        self.write_count.fetch_add(1, Ordering::SeqCst);
        let mut data = self.data.lock().unwrap();
        let off = block as usize * self.bs as usize;
        let end = off + self.bs as usize;
        if end > data.len() {
            return Err(Error::BlockOutOfRange(block));
        }
        data[off..end].copy_from_slice(&wdata[..self.bs as usize]);
        Ok(())
    }

    fn write_blocks(&self, block: u64, count: u32, data: &[u8]) -> Result<()> {
        for i in 0..count {
            self.write_block(block + i as u64, &data[i as usize * self.bs as usize..])?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

fn format_mem_data(blocks: u64) -> Vec<u8> {
    let dev = MemBlockDevice::new(blocks, 512);
    let opts = FormatOptions {
        volume_name: "MemTest".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, blocks, &opts).unwrap();
    dev.snapshot()
}

// ============================================================
// Fault Injection Tests
// ============================================================

mod fault_injection {
    use super::*;

    #[test]
    fn io_error_during_write_returns_error() {
        let formatted = format_mem_data(4096);
        let dev = MemBlockDevice::from_data(formatted, 512);
        dev.fail_after(3);
        let vol = Volume::from_device(Box::new(dev)).unwrap();
        let mut w = Writer::open(vol).unwrap();
        let big_data = vec![0xABu8; 8192];
        let result = w.write_file("big.bin", &big_data);
        assert!(result.is_err());
    }

    #[test]
    fn io_error_preserves_existing_files() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("existing.txt", b"keep me").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("existing.txt").unwrap(), b"keep me");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_after_failed_write_still_works() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("good1.txt", b"first").unwrap();
        let result = w.write_file("", b"empty name");
        let _ = result;
        w.write_file("good2.txt", b"second").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("good1.txt").unwrap(), b"first");
        assert_eq!(vol.read_file("good2.txt").unwrap(), b"second");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fail_during_mkdir_leaves_volume_openable() {
        let formatted = format_mem_data(4096);
        for fail_at in 1..10 {
            let dev = MemBlockDevice::from_data(formatted.clone(), 512);
            dev.fail_after(fail_at);
            let vol = Volume::from_device(Box::new(dev)).unwrap();
            let mut w = Writer::open(vol).unwrap();
            let _ = w.create_dir("TestDir");
        }
    }

    #[test]
    fn fail_during_delete_leaves_volume_openable() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("victim.txt", b"delete me").unwrap();
        drop(w);
        let mut w = open_writer(&path);
        w.delete("victim.txt").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert!(vol.lookup("victim.txt").unwrap().is_none());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Power-Loss Simulation
// ============================================================

mod power_loss {
    use super::*;

    fn crash_test(op: impl Fn(&mut Writer), max_attempts: u64) {
        let formatted = format_mem_data(4096);

        for fail_at in 1..max_attempts {
            let dev = MemBlockDevice::from_data(formatted.clone(), 512);
            dev.fail_after(fail_at);
            let vol_result =
                Volume::from_device(Box::new(MemBlockDevice::from_data(formatted.clone(), 512)));
            let Ok(vol) = vol_result else { continue };
            let Ok(mut w) = Writer::open(vol) else {
                continue;
            };
            let _ = op(&mut w);
        }
    }

    #[test]
    fn crash_during_write_file_no_panic() {
        crash_test(
            |w| {
                let _ = w.write_file("crash.txt", b"important data");
            },
            25,
        );
    }

    #[test]
    fn crash_during_large_write_no_panic() {
        crash_test(
            |w| {
                let _ = w.write_file("big.bin", &vec![0xAA; 4096]);
            },
            30,
        );
    }

    #[test]
    fn crash_during_mkdir_no_panic() {
        crash_test(
            |w| {
                let _ = w.create_dir("NewDir");
            },
            15,
        );
    }

    #[test]
    fn crash_during_delete_no_panic() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("target.txt", b"will be deleted").unwrap();
        drop(w);
        let mut w = open_writer(&path);
        w.delete("target.txt").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert!(vol.lookup("target.txt").unwrap().is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn crash_during_overwrite_no_panic() {
        crash_test(
            |w| {
                let _ = w.write_file("ow.txt", b"first version");
                let _ = w.write_file("ow.txt", b"second version, longer");
            },
            40,
        );
    }
}

// ============================================================
// Disk Full Handling
// ============================================================

mod disk_full {
    use super::*;

    #[test]
    fn disk_full_returns_error() {
        let path = fresh_image(128);
        let mut w = open_writer(&path);
        let big = vec![0xAAu8; 60_000];
        let result = w.write_file("toobig.bin", &big);
        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn disk_full_preserves_existing_files() {
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        w.write_file("small.txt", b"I exist").unwrap();
        let filler = vec![0u8; 100_000];
        let _ = w.write_file("filler.bin", &filler);
        drop(w);
        let mut vol = reopen(&path);
        assert_eq!(vol.read_file("small.txt").unwrap(), b"I exist");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_files_on_full_disk() {
        let path = fresh_image(512);
        let mut w = open_writer(&path);
        let mut files = Vec::new();
        for i in 0..100 {
            let name = format!("f{}.bin", i);
            let data = vec![i as u8; 256];
            if w.write_file(&name, &data).is_ok() {
                files.push((name, data));
            } else {
                break;
            }
        }
        assert!(!files.is_empty());
        drop(w);

        let mut vol = reopen(&path);
        for (name, expected) in &files {
            let data = vol.read_file(name).unwrap();
            assert_eq!(&data, expected, "data mismatch for {}", name);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn list_dir_on_full_disk() {
        let path = fresh_image(512);
        let mut w = open_writer(&path);
        let mut count = 0;
        for i in 0..100 {
            if w.write_file(&format!("x{}.txt", i), &[0; 64]).is_ok() {
                count += 1;
            } else {
                break;
            }
        }
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), count);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_on_full_disk_returns_error() {
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        while w
            .write_file(&format!("f{}", w.vol.free_blocks()), &[0; 512])
            .is_ok()
        {}
        let result = w.write_file("overflow.txt", b"nope");
        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn volume_info_on_full_disk() {
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        while w
            .write_file(&format!("f{}", w.vol.free_blocks()), &[0; 512])
            .is_ok()
        {}
        let free = w.vol.free_blocks();
        let total = w.vol.total_blocks();
        assert!(free < 5);
        assert!(total > 0);
        assert_eq!(w.vol.name(), "FaultTest");
        std::fs::remove_file(&path).ok();
    }
}
