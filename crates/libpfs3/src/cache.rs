//! Block cache for reserved blocks (dirblocks, anodeblocks, indexblocks).
//! O(1) access via HashMap, O(n) eviction on capacity overflow (rare at n=256).

use std::collections::HashMap;

use crate::error::Result;
use crate::io::BlockDevice;

const DEFAULT_CAPACITY: usize = 256;

struct CacheEntry {
    data: Vec<u8>,
    generation: u64,
}

/// LRU block cache for reserved-area blocks.
/// Uses a generation counter for O(1) access and O(n) eviction (rare).
pub struct BlockCache {
    entries: HashMap<u64, CacheEntry>,
    generation: u64,
    capacity: usize,
}

impl Default for BlockCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            generation: 0,
            capacity: DEFAULT_CAPACITY,
        }
    }

    /// Invalidate a cached block (after write).
    pub fn invalidate(&mut self, block: u64) {
        self.entries.remove(&block);
    }

    /// Read a reserved-area block, returning cached data if available.
    pub fn read_reserved(
        &mut self,
        dev: &dyn BlockDevice,
        block: u64,
        reserved_blksize: u16,
    ) -> Result<&[u8]> {
        self.generation += 1;
        let g = self.generation;

        if self.entries.contains_key(&block) {
            let entry = self.entries.get_mut(&block).unwrap();
            entry.generation = g;
            return Ok(&entry.data);
        }

        // Read from disk
        let sectors = ((reserved_blksize as u32) / dev.block_size()).max(1);
        let mut buf = vec![0u8; reserved_blksize as usize];
        dev.read_blocks(block, sectors, &mut buf)?;

        // Evict if at capacity
        if self.entries.len() >= self.capacity {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.generation)
                .map(|(&k, _)| k);
            if let Some(k) = oldest {
                self.entries.remove(&k);
            }
        }

        let entry = self.entries.entry(block).or_insert(CacheEntry {
            data: buf,
            generation: g,
        });
        Ok(&entry.data)
    }
}
