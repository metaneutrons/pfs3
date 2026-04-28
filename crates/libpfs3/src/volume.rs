//! PFS3 volume: top-level read-only access to a PFS3 partition.

use std::path::Path;

use crate::anode::AnodeReader;
use crate::bitmap::BitmapReader;
use crate::cache::BlockCache;
use crate::dir;
use crate::error::{Error, Result};
use crate::io::{BlockDevice, FileBlockDevice};
use crate::ondisk::*;
use crate::rdb::detect_pfs3_partition;

/// A mounted PFS3 volume providing read access to files and directories.
pub struct Volume {
    pub(crate) dev: Box<dyn BlockDevice>,
    pub(crate) cache: BlockCache,
    pub rootblock: Rootblock,
    pub rootblock_ext: Option<RootblockExt>,
    pub(crate) anodes: AnodeReader,
    pub(crate) bitmap: BitmapReader,
}

impl Volume {
    /// Validate reserved block size is sane (prevents div-by-zero downstream).
    fn validate_rbs(rb: &Rootblock) -> Result<()> {
        if rb.reserved_blksize < 64 {
            return Err(Error::Corrupt(format!(
                "reserved_blksize {} too small (minimum 64)",
                rb.reserved_blksize
            )));
        }
        Ok(())
    }

    /// Open a PFS3 volume from an already-opened block device.
    pub fn from_device(dev: Box<dyn BlockDevice>) -> Result<Self> {
        let mut buf = vec![0u8; 512];
        dev.read_block(ROOTBLOCK, &mut buf)?;
        let rb = Rootblock::parse(&buf)?;

        let rblk_bytes = rb.rblkcluster as u32 * 512;
        let rootblock = if rblk_bytes > 512 {
            let mut big_buf = vec![0u8; rblk_bytes as usize];
            dev.read_blocks(ROOTBLOCK, rb.rblkcluster as u32, &mut big_buf)?;
            Rootblock::parse(&big_buf)?
        } else {
            rb
        };

        let mut cache = BlockCache::new();
        let rootblock_ext = if rootblock.has_extension() {
            let rbs = rootblock.reserved_blksize;
            let data = cache.read_reserved(dev.as_ref(), rootblock.extension as u64, rbs)?;
            Some(RootblockExt::parse(data)?)
        } else {
            None
        };

        let anodes = AnodeReader::new(&rootblock, rootblock_ext.as_ref());
        let bitmap = BitmapReader::new(&rootblock);
        Self::validate_rbs(&rootblock)?;

        Ok(Self {
            dev,
            cache,
            rootblock,
            rootblock_ext,
            anodes,
            bitmap,
        })
    }

    /// Open a PFS3 volume from a file.
    /// `partition_offset` is the byte offset to the partition start (0 for raw PFS3 images).
    pub fn open(path: &Path, partition_offset: u64) -> Result<Self> {
        Self::open_impl(path, partition_offset, false)
    }

    /// Open a PFS3 volume for read-write access.
    pub fn open_rw(path: &Path, partition_offset: u64) -> Result<Self> {
        Self::open_impl(path, partition_offset, true)
    }

    fn open_impl(path: &Path, partition_offset: u64, writable: bool) -> Result<Self> {
        let dev = if writable {
            FileBlockDevice::open_rw(path, 512, partition_offset, 0)?
        } else {
            FileBlockDevice::open(path, 512, partition_offset, 0)?
        };
        Self::from_device(Box::new(dev))
    }

    /// Open a PFS3 volume from an RDB disk image, auto-detecting the partition offset.
    pub fn open_rdb(path: &Path) -> Result<Self> {
        let offset = detect_pfs3_partition(path)?;
        Self::open(path, offset)
    }

    /// Open a specific named partition from an RDB disk image (e.g. "DH0").
    pub fn open_partition(path: &Path, name: &str) -> Result<Self> {
        Self::open(path, Self::find_partition_offset(path, name)?)
    }

    /// Open an RDB disk image for read-write access.
    pub fn open_rdb_rw(path: &Path) -> Result<Self> {
        let offset = detect_pfs3_partition(path)?;
        Self::open_rw(path, offset)
    }

    /// Open a named partition for read-write access.
    pub fn open_partition_rw(path: &Path, name: &str) -> Result<Self> {
        Self::open_rw(path, Self::find_partition_offset(path, name)?)
    }

    /// Open a volume with automatic partition/RDB detection.
    /// Unified entry point for CLI commands.
    pub fn open_auto(
        path: &Path,
        offset: u64,
        partition: Option<&str>,
        writable: bool,
    ) -> Result<Self> {
        let open_fn = if writable {
            Self::open_rw as fn(&Path, u64) -> Result<Self>
        } else {
            Self::open as fn(&Path, u64) -> Result<Self>
        };
        if let Some(name) = partition {
            open_fn(path, Self::find_partition_offset(path, name)?)
        } else if offset == 0 {
            let off = detect_pfs3_partition(path).unwrap_or(0);
            open_fn(path, off)
        } else {
            open_fn(path, offset)
        }
    }

    fn find_partition_offset(path: &Path, name: &str) -> Result<u64> {
        let parts = detect_pfs3_partitions(path)?;
        let part = parts
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .or_else(|| name.parse::<usize>().ok().and_then(|i| parts.get(i)))
            .ok_or_else(|| {
                let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
                Error::NotFound(format!(
                    "partition '{}' not found (available: {})",
                    name,
                    names.join(", ")
                ))
            })?;
        Ok(part.offset)
    }

    // --- Info ---

    /// Volume name from the rootblock.
    pub fn name(&self) -> &str {
        &self.rootblock.diskname
    }

    /// Total blocks on disk.
    pub fn total_blocks(&self) -> u32 {
        self.rootblock.disksize
    }

    /// Number of free data blocks.
    pub fn free_blocks(&self) -> u32 {
        self.rootblock.blocksfree
    }

    /// Block size in bytes.
    pub fn block_size(&self) -> u32 {
        self.dev.block_size()
    }

    /// Maximum filename length (32 or 107 with long filenames).
    pub fn fnsize(&self) -> u16 {
        self.rootblock_ext.as_ref().map(|e| e.fnsize).unwrap_or(32)
    }

    /// Count free blocks by scanning the data bitmap.
    pub fn bitmap_count_free(&mut self) -> Result<u32> {
        self.bitmap
            .count_free(self.dev.as_ref(), &mut self.cache, self.rootblock.disksize)
    }

    /// Get the anode chain for a given anode number (for check/debug).
    pub fn get_anode_chain(&mut self, anodenr: u32) -> Result<Vec<crate::ondisk::Anode>> {
        self.anodes
            .get_chain(anodenr, self.dev.as_ref(), &mut self.cache)
    }

    /// Count free reserved blocks by scanning the reserved bitmap in the rootblock cluster.
    pub fn reserved_count_free(&mut self) -> Result<u32> {
        let bs = self.block_size() as usize;
        let rblkcluster = self.rootblock.rblkcluster as u32;
        let cluster_size = rblkcluster as usize * bs;
        let mut cluster = vec![0u8; cluster_size];
        self.dev.read_blocks(
            self.rootblock.firstreserved as u64,
            rblkcluster,
            &mut cluster,
        )?;
        let bm_off = bs + 12; // bitmap starts after rootblock + 12-byte header
        let mut free = 0u32;
        let mut i = bm_off;
        while i + 4 <= cluster.len() {
            let word = u32::from_be_bytes(cluster[i..i + 4].try_into().unwrap());
            free += word.count_ones();
            i += 4;
        }
        Ok(free)
    }

    // --- Directory operations ---

    /// List directory entries at the given path.
    pub fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let dir_anode = dir::resolve_dir_path(
            path,
            &self.anodes,
            self.dev.as_ref(),
            &mut self.cache,
            self.rootblock.reserved_blksize,
        )?;
        self.list_dir_by_anode(dir_anode)
    }

    /// List directory entries by anode number.
    pub fn list_dir_by_anode(&mut self, dir_anode: u32) -> Result<Vec<DirEntry>> {
        dir::list_entries(
            dir_anode,
            &self.anodes,
            self.dev.as_ref(),
            &mut self.cache,
            self.rootblock.reserved_blksize,
        )
    }

    /// Look up a directory entry by path. Returns `None` for root.
    pub fn lookup(&mut self, path: &str) -> Result<Option<DirEntry>> {
        dir::resolve_path(
            path,
            &self.anodes,
            self.dev.as_ref(),
            &mut self.cache,
            self.rootblock.reserved_blksize,
        )
    }

    // --- File reading ---

    /// Read a file's contents by path.
    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .lookup(path)?
            .ok_or_else(|| Error::NotFound(path.to_string()))?;
        if entry.is_dir() {
            return Err(Error::NotADirectory);
        }
        self.read_file_data(entry.anode, entry.file_size())
    }

    /// Read file data by anode number and known size.
    pub fn read_file_data(&mut self, anodenr: u32, size: u64) -> Result<Vec<u8>> {
        let chain = self
            .anodes
            .get_chain(anodenr, self.dev.as_ref(), &mut self.cache)?;
        let bs = self.dev.block_size() as u64;
        // Cap allocation to what the anode chain can actually provide,
        // protecting against corrupt size fields that would cause OOM.
        let chain_capacity: u64 = chain.iter().map(|a| a.clustersize as u64 * bs).sum();
        let actual_size = size.min(chain_capacity);
        let mut data = Vec::with_capacity(actual_size as usize);
        let mut remaining = size;

        let mut block_buf = vec![0u8; bs as usize];
        for an in &chain {
            for i in 0..an.clustersize {
                if remaining == 0 {
                    break;
                }
                let blk = an.blocknr as u64 + i as u64;
                self.dev.read_block(blk, &mut block_buf)?;
                let chunk = (remaining).min(bs) as usize;
                data.extend_from_slice(&block_buf[..chunk]);
                remaining -= chunk as u64;
            }
        }
        Ok(data)
    }

    /// Read a byte range from a file without loading the entire file.
    /// Only reads the blocks that overlap [offset, offset+length).
    pub fn read_file_range(
        &mut self,
        anodenr: u32,
        file_size: u64,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>> {
        if offset >= file_size {
            return Ok(Vec::new());
        }
        let end = (offset + length as u64).min(file_size);
        let bs = self.dev.block_size() as u64;
        let chain = self
            .anodes
            .get_chain(anodenr, self.dev.as_ref(), &mut self.cache)?;

        let mut result = Vec::with_capacity((end - offset) as usize);
        let mut block_pos: u64 = 0; // byte position of current extent start
        let mut block_buf = vec![0u8; bs as usize];

        for an in &chain {
            let extent_bytes = an.clustersize as u64 * bs;
            let extent_end = block_pos + extent_bytes;

            // Skip extents entirely before our range
            if extent_end <= offset {
                block_pos = extent_end;
                continue;
            }
            // Stop if we've read enough
            if block_pos >= end {
                break;
            }

            for i in 0..an.clustersize {
                let blk_start = block_pos + i as u64 * bs;
                let blk_end = blk_start + bs;
                if blk_end <= offset {
                    continue;
                }
                if blk_start >= end {
                    break;
                }
                self.dev
                    .read_block(an.blocknr as u64 + i as u64, &mut block_buf)?;
                let slice_start = if offset > blk_start {
                    (offset - blk_start) as usize
                } else {
                    0
                };
                let slice_end = if end < blk_end {
                    (end - blk_start) as usize
                } else {
                    bs as usize
                };
                result.extend_from_slice(&block_buf[slice_start..slice_end]);
            }
            block_pos = extent_end;
        }
        Ok(result)
    }
    /// Walk an anode chain and return all data block numbers. For fsck.
    pub fn validate_anode_chain(&mut self, anodenr: u32) -> Result<Vec<u64>> {
        let chain = self
            .anodes
            .get_chain(anodenr, self.dev.as_ref(), &mut self.cache)?;
        let mut blocks = Vec::new();
        for an in &chain {
            for i in 0..an.clustersize {
                blocks.push(an.blocknr as u64 + i as u64);
            }
        }
        Ok(blocks)
    }

    /// List deleted files from the deldir (trash).
    pub fn list_deldir(&mut self) -> Result<Vec<DelDirEntry>> {
        if !self.rootblock.has_flag(MODE_DELDIR) {
            return Ok(Vec::new());
        }
        let rext = match &self.rootblock_ext {
            Some(e) => e,
            None => return Ok(Vec::new()),
        };
        let rbs = self.rootblock.reserved_blksize;
        let entries_per_block = deldir_entries_per_block(rbs);
        let mut result = Vec::new();
        for &blk in &rext.deldirblocks {
            if blk == 0 {
                continue;
            }
            let data = self
                .cache
                .read_reserved(self.dev.as_ref(), blk as u64, rbs)?;
            if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DELDIRID {
                continue;
            }
            for i in 0..entries_per_block {
                let off = DELDIR_HEADER_SIZE + i * DELDIR_ENTRY_SIZE;
                if off + DELDIR_ENTRY_SIZE <= data.len()
                    && let Some(entry) = DelDirEntry::parse(&data[off..off + DELDIR_ENTRY_SIZE])
                {
                    result.push(entry);
                }
            }
        }
        Ok(result)
    }
}

// Re-export for backward compatibility
pub use crate::rdb::{PartitionInfo, detect_pfs3_partitions};
