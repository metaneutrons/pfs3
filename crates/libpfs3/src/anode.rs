//! Anode (extent) lookup and chain traversal.
//!
//! Anode addressing:
//!   Split mode: anodenr = (seqnr << 16) | offset
//!   Non-split:  anodenr = seqnr * anodes_per_block + offset
//!
//! Lookup path (small disk):
//!   rootblock.indexblocks[seqnr / ipb] → index block → anode block
//!
//! Lookup path (large disk, MODE_SUPERINDEX):
//!   rootblockext.superindex[seqnr / ipb²] → super block → index block → anode block

use std::collections::HashSet;

use crate::cache::BlockCache;
use crate::error::{Error, Result};
use crate::io::BlockDevice;
use crate::ondisk::*;

/// Reads anode (extent) records from the reserved area.
pub struct AnodeReader {
    reserved_blksize: u16,
    anodes_per_block: u32,
    index_per_block: u32,
    split_mode: bool,
    is_large: bool,
    indexblocks: Vec<u32>,
    superindex: Vec<u32>,
}

impl AnodeReader {
    /// Create a new reader from rootblock and optional extension.
    pub fn new(rb: &Rootblock, rbe: Option<&RootblockExt>) -> Self {
        let rbs = rb.reserved_blksize;
        Self {
            reserved_blksize: rbs,
            anodes_per_block: (rbs as u32).saturating_sub(ANODE_BLOCK_HEADER_SIZE as u32)
                / ANODE_SIZE as u32,
            index_per_block: (rbs as u32 / 4).saturating_sub(3),
            split_mode: rb.is_splitted_anodes(),
            is_large: rb.is_large(),
            indexblocks: rb.indexblocks.clone(),
            superindex: rbe.map(|e| e.superindex.clone()).unwrap_or_default(),
        }
    }

    /// Look up a single anode by number.
    pub fn get_anode(
        &self,
        anodenr: u32,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
    ) -> Result<Anode> {
        let (seqnr, offset) = if self.split_mode {
            (anodenr >> 16, anodenr & 0xFFFF)
        } else {
            (
                anodenr / self.anodes_per_block,
                anodenr % self.anodes_per_block,
            )
        };

        let blk_nr = self.resolve_anode_block(seqnr, dev, cache)?;
        if blk_nr == 0 {
            return Err(Error::AnodeNotFound(anodenr));
        }

        let data = cache.read_reserved(dev, blk_nr as u64, self.reserved_blksize)?;
        AnodeBlockHeader::parse(data)?;

        let base = ANODE_BLOCK_HEADER_SIZE + offset as usize * ANODE_SIZE;
        if base + ANODE_SIZE > data.len() {
            return Err(Error::AnodeNotFound(anodenr));
        }
        Anode::parse(&data[base..base + ANODE_SIZE], anodenr)
    }

    /// Follow an anode chain from `anodenr` to EOF. Detects cycles.
    pub fn get_chain(
        &self,
        anodenr: u32,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
    ) -> Result<Vec<Anode>> {
        const MAX_CHAIN_LEN: usize = 1_000_000;
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut nr = anodenr;
        while nr != ANODE_EOF {
            if !seen.insert(nr) {
                return Err(Error::InvalidPartition(format!("anode cycle at {}", nr)));
            }
            if chain.len() >= MAX_CHAIN_LEN {
                return Err(Error::InvalidPartition("anode chain too long".into()));
            }
            let an = self.get_anode(nr, dev, cache)?;
            nr = an.next;
            chain.push(an);
        }
        Ok(chain)
    }

    /// Resolve an anode sequence number to its on-disk block number.
    pub fn resolve_anode_block(
        &self,
        seqnr: u32,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
    ) -> Result<u32> {
        let ipb = self.index_per_block;
        if self.is_large {
            // superindex → index → anode block
            let super_nr = seqnr / (ipb * ipb);
            let remainder = seqnr % (ipb * ipb);
            let idx_nr = remainder / ipb;
            let idx_off = remainder % ipb;

            let super_blk = *self.superindex.get(super_nr as usize).unwrap_or(&0);
            if super_blk == 0 {
                return Ok(0);
            }
            let idx_blk = self.read_index_entry(super_blk, idx_nr, dev, cache)?;
            if idx_blk == 0 {
                return Ok(0);
            }
            self.read_index_entry(idx_blk, idx_off, dev, cache)
        } else {
            // indexblocks → index → anode block
            let idx_nr = seqnr / ipb;
            let idx_off = seqnr % ipb;
            let idx_blk = *self.indexblocks.get(idx_nr as usize).unwrap_or(&0);
            if idx_blk == 0 {
                return Ok(0);
            }
            self.read_index_entry(idx_blk, idx_off, dev, cache)
        }
    }

    fn read_index_entry(
        &self,
        block: u32,
        offset: u32,
        dev: &dyn BlockDevice,
        cache: &mut BlockCache,
    ) -> Result<u32> {
        let data = cache.read_reserved(dev, block as u64, self.reserved_blksize)?;
        let off = INDEX_BLOCK_HEADER_SIZE + offset as usize * 4;
        if off + 4 > data.len() {
            return Ok(0);
        }
        Ok(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()))
    }
}
