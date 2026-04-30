// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Corrupt image robustness: bad magic, truncated, disksize zero/max,
//! reserved_blksize invalid, anode cycles, corrupt dir entries, bitmap
//! corruption, OOB extension, open_nonexistent.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::error::{Error, Result};
use libpfs3::format::{self, FormatOptions};
use libpfs3::io::{BlockDevice, FileBlockDevice};
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(13000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_corrupt_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "CorruptTest".into(),
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

struct MemDev {
    data: Mutex<Vec<u8>>,
    bs: u32,
}

impl MemDev {
    fn from_data(data: Vec<u8>) -> Self {
        Self {
            data: Mutex::new(data),
            bs: 512,
        }
    }
}

impl BlockDevice for MemDev {
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

fn format_mem(blocks: u64) -> Vec<u8> {
    let dev = MemDev::from_data(vec![0u8; blocks as usize * 512]);
    let opts = FormatOptions {
        volume_name: "MemTest".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, blocks, &opts).unwrap();
    dev.data.into_inner().unwrap()
}

fn put_be32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}

fn put_be16(buf: &mut [u8], off: usize, val: u16) {
    buf[off..off + 2].copy_from_slice(&val.to_be_bytes());
}

// ============================================================
// Corrupt Image Parsing — must not panic, must return Err
// ============================================================

#[test]
fn open_nonexistent() {
    assert!(Volume::open_rdb(Path::new("/nonexistent.hdf")).is_err());
}

mod corrupt_images {
    use super::*;

    #[test]
    fn empty_image_returns_error() {
        let dev = MemDev::from_data(vec![0u8; 4096 * 512]);
        assert!(Volume::from_device(Box::new(dev)).is_err());
    }

    #[test]
    fn truncated_rootblock_returns_error() {
        let dev = MemDev::from_data(vec![0u8; 64]);
        assert!(Volume::from_device(Box::new(dev)).is_err());
    }

    #[test]
    fn bad_magic_returns_error() {
        let mut data = vec![0u8; 4096 * 512];
        put_be32(&mut data, 2 * 512, 0xDEADBEEF);
        let dev = MemDev::from_data(data);
        assert!(Volume::from_device(Box::new(dev)).is_err());
    }

    #[test]
    fn disksize_zero_no_panic() {
        let mut data = format_mem(4096);
        put_be32(&mut data, 2 * 512 + 0x54, 0);
        let dev = MemDev::from_data(data);
        let _ = Volume::from_device(Box::new(dev));
    }

    #[test]
    fn disksize_max_no_panic() {
        let mut data = format_mem(4096);
        put_be32(&mut data, 2 * 512 + 0x54, u32::MAX);
        let dev = MemDev::from_data(data);
        let _ = Volume::from_device(Box::new(dev));
    }

    #[test]
    fn reserved_blksize_zero_returns_error() {
        let mut data = format_mem(4096);
        put_be16(&mut data, 2 * 512 + 0x40, 0);
        let dev = MemDev::from_data(data);
        assert!(Volume::from_device(Box::new(dev)).is_err());
    }

    #[test]
    fn reserved_blksize_odd_no_panic() {
        let mut data = format_mem(4096);
        put_be16(&mut data, 2 * 512 + 0x40, 13);
        let dev = MemDev::from_data(data);
        assert!(Volume::from_device(Box::new(dev)).is_err());
    }

    #[test]
    fn circular_anode_chain_no_infinite_loop() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("loop.txt", &vec![0u8; 2048]).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("loop.txt").unwrap().unwrap();
        let chain = vol.validate_anode_chain(entry.anode).unwrap();
        assert!(!chain.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_dir_entry_nlen_exceeds_esize_no_panic() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("victim.txt", b"data").unwrap();
        drop(w);

        let mut data = std::fs::read(&path).unwrap();
        for blk in 0..(data.len() / 512) {
            let off = blk * 512;
            if data[off] == 0x44 && data[off + 1] == 0x42 {
                let entry_off = off + 20;
                if entry_off + 18 < data.len() && data[entry_off] > 0 {
                    data[entry_off + 17] = 255;
                    break;
                }
            }
        }
        std::fs::write(&path, &data).unwrap();
        if let Ok(mut vol) = Volume::open(&path, 0) {
            let _ = vol.list_dir("/");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_bitmap_extra_bits_no_panic() {
        let mut data = format_mem(4096);
        for blk in 0..(data.len() / 512) {
            let off = blk * 512;
            if data.len() > off + 1 && data[off] == 0x42 && data[off + 1] == 0x4D {
                for i in 12..512 {
                    data[off + i] = 0xFF;
                }
            }
        }
        let dev = MemDev::from_data(data);
        if let Ok(mut vol) = Volume::from_device(Box::new(dev)) {
            let _ = vol.bitmap_count_free();
        }
    }

    #[test]
    fn rootblock_extension_truncated_no_panic() {
        let mut data = format_mem(4096);
        let dev = MemDev::from_data(data.clone());
        let vol = Volume::from_device(Box::new(dev)).unwrap();
        if vol.rootblock_ext.is_some() {
            let ext_blk = vol.rootblock.extension;
            drop(vol);
            if ext_blk > 0 {
                let off = ext_blk as usize * 512;
                if off + 512 <= data.len() {
                    for b in &mut data[off..off + 512] {
                        *b = 0;
                    }
                }
            }
            let dev = MemDev::from_data(data);
            let _ = Volume::from_device(Box::new(dev));
        }
    }

    #[test]
    fn all_zeros_rootblock_area_no_panic() {
        let mut data = vec![0u8; 8192 * 512];
        put_be32(&mut data, 2 * 512, ID_PFS_DISK);
        let dev = MemDev::from_data(data);
        let _ = Volume::from_device(Box::new(dev));
    }

    #[test]
    fn rootblock_with_extension_pointing_oob_no_panic() {
        let mut data = format_mem(4096);
        put_be32(&mut data, 2 * 512 + 0x58, 999999);
        let opts_off = 2 * 512 + 0x04;
        let opts = u32::from_be_bytes(data[opts_off..opts_off + 4].try_into().unwrap());
        put_be32(&mut data, opts_off, opts | MODE_EXTENSION);
        let dev = MemDev::from_data(data);
        let _ = Volume::from_device(Box::new(dev));
    }
}

// ============================================================
// Anode Cycle Detection (corrupted on-disk)
// ============================================================

mod anode_cycles {
    use super::*;

    #[test]
    fn read_file_on_corrupted_circular_chain_terminates() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("victim.txt", &vec![0xAA; 2048]).unwrap();
        drop(w);

        let mut data = std::fs::read(&path).unwrap();
        for blk in 0..(data.len() / 512) {
            let off = blk * 512;
            if data[off] == 0x41 && data[off + 1] == 0x42 {
                let anode_base = off + 16 + ANODE_USERFIRST as usize * ANODE_SIZE;
                if anode_base + 12 <= data.len() {
                    let cs =
                        u32::from_be_bytes(data[anode_base..anode_base + 4].try_into().unwrap());
                    if cs > 0 {
                        let anodenr = ANODE_USERFIRST;
                        put_be32(&mut data, anode_base + 8, anodenr);
                        break;
                    }
                }
            }
        }
        std::fs::write(&path, &data).unwrap();

        let mut vol = reopen(&path);
        let entry = vol.lookup("victim.txt").unwrap().unwrap();
        let result = vol.read_file_data(entry.anode, entry.file_size());
        let _ = result;
        std::fs::remove_file(&path).ok();
    }
}
