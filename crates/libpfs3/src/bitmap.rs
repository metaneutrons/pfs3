//! Bitmap reading for free block counting (statfs).
//!
//! For read-only access, we just use the rootblock's blocksfree field.
//! Full bitmap scanning is for fsck (pfs3 check).

use crate::cache::BlockCache;
use crate::error::Result;
use crate::io::BlockDevice;
use crate::ondisk::*;

/// Bitmap index reader — resolves bitmap block sequence numbers.
pub struct BitmapReader {
    reserved_blksize: u16,
    index_per_block: u32,
    bitmapindex: Vec<u32>,
}

impl BitmapReader {
    /// Create a new reader from rootblock metadata.
    pub fn new(rb: &Rootblock) -> Self {
        Self {
            reserved_blksize: rb.reserved_blksize,
            index_per_block: rb.index_per_block(),
            bitmapindex: rb.bitmapindex.clone(),
        }
    }

    /// Count free blocks by scanning all bitmap blocks.
    pub fn count_free(
        &self,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
        disksize: u32,
    ) -> Result<u32> {
        let mut free = 0u32;
        let mut blocks_remaining = disksize;

        let mut seqnr = 0u32;
        while blocks_remaining > 0 {
            if let Some(blk) = self.get_bitmap_block(seqnr, dev, cache)? {
                let data = cache.read_reserved(dev, blk as u64, self.reserved_blksize)?;
                // Bitmap data starts at offset 12
                let bm_start = BITMAP_BLOCK_HEADER_SIZE;
                let mut pos = bm_start;
                while pos + 4 <= data.len() && blocks_remaining > 0 {
                    let word = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
                    let bits = blocks_remaining.min(32);
                    if bits == 32 {
                        free += word.count_ones();
                    } else {
                        // Only count the relevant bits
                        let mask = !0u32 << (32 - bits);
                        free += (word & mask).count_ones();
                    }
                    blocks_remaining = blocks_remaining.saturating_sub(32);
                    pos += 4;
                }
            } else {
                break;
            }
            seqnr += 1;
        }
        Ok(free)
    }

    pub fn get_bitmap_block(
        &self,
        seqnr: u32,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
    ) -> Result<Option<u32>> {
        let ipb = self.index_per_block;
        let idx_nr = seqnr / ipb;
        let idx_off = seqnr % ipb;

        let bmi_blk = *self.bitmapindex.get(idx_nr as usize).unwrap_or(&0);
        if bmi_blk == 0 {
            return Ok(None);
        }

        let data = cache.read_reserved(dev, bmi_blk as u64, self.reserved_blksize)?;
        let off = INDEX_BLOCK_HEADER_SIZE + idx_off as usize * 4;
        if off + 4 > data.len() {
            return Ok(None);
        }
        let v = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
        Ok(if v != 0 { Some(v) } else { None })
    }
}
