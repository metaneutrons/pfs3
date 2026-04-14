//! Block device abstraction.
//!
//! Provides a trait for reading/writing blocks and a file-backed implementation
//! that handles partition offsets within RDB disk images.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use crate::error::{Error, Result};

/// Abstract block device for reading/writing fixed-size blocks.
pub trait BlockDevice: Send + Sync {
    /// Read a single block into `buf`.
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<()>;
    /// Read `count` consecutive blocks into `buf`.
    fn read_blocks(&self, block: u64, count: u32, buf: &mut [u8]) -> Result<()>;
    /// Return the block size in bytes.
    fn block_size(&self) -> u32;
    /// Write a single block from `data`.
    fn write_block(&self, block: u64, data: &[u8]) -> Result<()>;
    /// Write `count` consecutive blocks from `data`.
    fn write_blocks(&self, block: u64, count: u32, data: &[u8]) -> Result<()>;
    /// Flush all pending writes to stable storage.
    fn flush(&self) -> Result<()>;
}

/// File-backed block device with optional partition byte offset.
pub struct FileBlockDevice {
    file: Mutex<File>,
    block_bytes: u32,
    partition_offset: u64,
    total_blocks: u64,
    writable: bool,
}

impl FileBlockDevice {
    /// Open a file as a read-only block device.
    pub fn open(
        path: &std::path::Path,
        block_bytes: u32,
        partition_offset: u64,
        total_blocks: u64,
    ) -> Result<Self> {
        Self::new_impl(
            File::open(path)?,
            block_bytes,
            partition_offset,
            total_blocks,
            false,
        )
    }

    /// Open a file as a read-write block device.
    pub fn open_rw(
        path: &std::path::Path,
        block_bytes: u32,
        partition_offset: u64,
        total_blocks: u64,
    ) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Self::new_impl(file, block_bytes, partition_offset, total_blocks, true)
    }

    fn new_impl(
        file: File,
        block_bytes: u32,
        partition_offset: u64,
        total_blocks: u64,
        writable: bool,
    ) -> Result<Self> {
        let file_len = file.metadata()?.len();
        let total_blocks = if total_blocks == 0 {
            (file_len - partition_offset) / block_bytes as u64
        } else {
            total_blocks
        };
        Ok(Self {
            file: Mutex::new(file),
            block_bytes,
            partition_offset,
            total_blocks,
            writable,
        })
    }

    /// Create a new file of given size and open as read-write.
    pub fn create(path: &std::path::Path, block_bytes: u32, total_blocks: u64) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        let size = total_blocks * block_bytes as u64;
        file.set_len(size)?;
        Ok(Self {
            file: Mutex::new(file),
            block_bytes,
            partition_offset: 0,
            total_blocks,
            writable: true,
        })
    }

    /// Total number of blocks on the device.
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }
}

impl BlockDevice for FileBlockDevice {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<()> {
        self.read_blocks(block, 1, buf)
    }

    fn read_blocks(&self, block: u64, count: u32, buf: &mut [u8]) -> Result<()> {
        let offset = self.partition_offset + block * self.block_bytes as u64;
        let len = count as usize * self.block_bytes as usize;
        if buf.len() < len {
            return Err(Error::TooShort("read buffer"));
        }
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf[..len])?;
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.block_bytes
    }

    fn write_block(&self, block: u64, data: &[u8]) -> Result<()> {
        self.write_blocks(block, 1, data)
    }

    fn write_blocks(&self, block: u64, count: u32, data: &[u8]) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        let offset = self.partition_offset + block * self.block_bytes as u64;
        let len = count as usize * self.block_bytes as usize;
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&data[..len])?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        let file = self.file.lock().unwrap();
        file.sync_all()?;
        Ok(())
    }
}
