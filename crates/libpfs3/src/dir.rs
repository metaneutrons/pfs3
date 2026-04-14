//! Directory block reading and entry iteration.
//!
//! Directories are stored as chains of dirblocks (id='DB').
//! Each dirblock contains packed variable-length direntries.

use crate::anode::AnodeReader;
use crate::cache::BlockCache;
use crate::error::{Error, Result};
use crate::io::BlockDevice;
use crate::ondisk::*;
use crate::util;

/// List all entries in a directory given its anode number.
pub fn list_entries(
    dir_anode: u32,
    anodes: &AnodeReader,
    dev: &dyn BlockDevice,
    cache: &mut BlockCache,
    reserved_blksize: u16,
) -> Result<Vec<DirEntry>> {
    let chain = anodes.get_chain(dir_anode, dev, cache)?;
    let mut entries = Vec::new();

    for an in &chain {
        for i in 0..an.clustersize {
            let blk = an.blocknr as u64 + i as u64;
            let data = cache.read_reserved(dev, blk, reserved_blksize)?;
            if data.len() < DIR_BLOCK_HEADER_SIZE + 1 {
                continue;
            }
            let id = u16::from_be_bytes(data[0..2].try_into().unwrap());
            if id != DBLKID {
                continue;
            }
            parse_dirblock_entries(data, &mut entries);
        }
    }
    Ok(entries)
}

/// Parse all direntries from a dirblock's entry area.
fn parse_dirblock_entries(data: &[u8], entries: &mut Vec<DirEntry>) {
    let mut offset = DIR_BLOCK_HEADER_SIZE;
    while offset < data.len() {
        match DirEntry::parse(data, offset) {
            Some((entry, next)) => {
                entries.push(entry);
                offset = next;
            }
            None => break,
        }
    }
}

/// Look up a single name in a directory (case-insensitive).
pub fn lookup(
    dir_anode: u32,
    name: &str,
    anodes: &AnodeReader,
    dev: &dyn BlockDevice,
    cache: &mut BlockCache,
    reserved_blksize: u16,
) -> Result<DirEntry> {
    let entries = list_entries(dir_anode, anodes, dev, cache, reserved_blksize)?;
    entries
        .into_iter()
        .find(|e| util::name_eq_ci(&e.name, name))
        .ok_or_else(|| Error::NotFound(name.to_string()))
}

/// Resolve a '/'-separated path to a DirEntry.
/// Returns None for the root directory itself.
pub fn resolve_path(
    path: &str,
    anodes: &AnodeReader,
    dev: &dyn BlockDevice,
    cache: &mut BlockCache,
    reserved_blksize: u16,
) -> Result<Option<DirEntry>> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Ok(None); // root
    }

    let mut dir_anode = ANODE_ROOTDIR;
    for (i, part) in parts.iter().enumerate() {
        let entry = lookup(dir_anode, part, anodes, dev, cache, reserved_blksize)?;
        if i < parts.len() - 1 {
            if !entry.is_dir() {
                return Err(Error::NotADirectory);
            }
            dir_anode = entry.anode;
        } else {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

/// Resolve a path to a directory anode number.
pub fn resolve_dir_path(
    path: &str,
    anodes: &AnodeReader,
    dev: &dyn BlockDevice,
    cache: &mut BlockCache,
    reserved_blksize: u16,
) -> Result<u32> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Ok(ANODE_ROOTDIR);
    }

    let mut dir_anode = ANODE_ROOTDIR;
    for part in &parts {
        let entry = lookup(dir_anode, part, anodes, dev, cache, reserved_blksize)?;
        if !entry.is_dir() {
            return Err(Error::NotADirectory);
        }
        dir_anode = entry.anode;
    }
    Ok(dir_anode)
}
