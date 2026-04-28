//! Writable PFS3 volume — file/directory creation and deletion.
//!
//! Builds on the read-only Volume and adds:
//! - Data block allocation/free via bitmap
//! - Reserved block allocation via reserved bitmap
//! - Anode allocation and chain building
//! - Directory entry creation and removal
//! - Rootblock update

use crate::error::{Error, Result};
use crate::ondisk::*;
use crate::volume::Volume;

/// Writable PFS3 volume — file/directory creation, deletion, and formatting.
pub struct Writer {
    /// The underlying read-only volume (also used for writes to disk).
    pub vol: Volume,
    // Geometry
    resblocksize: u32,
    rescluster: u32,
    index_per_block: u32,
    anodes_per_block: u32,
    firstreserved: u32,
    numreserved: u32,
    bitmapstart: u32,
    datestamp: u32,
    // Mutable state
    res_bitmap: Vec<u32>,
    data_bm: Vec<(u32, Vec<u32>)>, // (blk_num, longs)
    /// Pending reserved block writes, flushed atomically before rootblock update.
    pending_writes: Vec<(u32, Vec<u8>)>,
}

impl Writer {
    /// Open a volume for writing.
    pub fn open(vol: Volume) -> Result<Self> {
        let rb = &vol.rootblock;
        let rbs = rb.reserved_blksize as u32;
        let rescluster = rbs / vol.block_size();
        let firstreserved = rb.firstreserved;
        let numreserved = (rb.lastreserved - firstreserved + 1) / rescluster;
        let index_per_block = (rbs / 4).saturating_sub(3);
        let anodes_per_block =
            rbs.saturating_sub(ANODE_BLOCK_HEADER_SIZE as u32) / ANODE_SIZE as u32;
        let bitmapstart = rb.lastreserved + 1;
        let datestamp = rb.datestamp;

        let mut w = Self {
            resblocksize: rbs,
            rescluster,
            index_per_block,
            anodes_per_block,
            firstreserved,
            numreserved,
            bitmapstart,
            datestamp,
            res_bitmap: Vec::new(),
            data_bm: Vec::new(),
            pending_writes: Vec::new(),
            vol,
        };
        w.load_reserved_bitmap()?;
        w.load_data_bitmap()?;
        Ok(w)
    }

    /// Consume the writer and return the underlying volume.
    pub fn into_volume(self) -> Volume {
        self.vol
    }

    fn next_datestamp(&mut self) -> u32 {
        self.datestamp += 1;
        self.datestamp
    }

    // ---- High-level API (path-based, for CLI) ----

    /// Write a file at the given path (creates parent dirs as needed).
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        let (parent_anode, filename) = self.split_path(path)?;
        self.write_file_in(parent_anode, &filename, data)
    }

    /// Create a directory at the given path.
    pub fn create_dir(&mut self, path: &str) -> Result<()> {
        let (parent_anode, dirname) = self.split_path(path)?;
        self.create_dir_in(parent_anode, &dirname)
    }

    /// Delete a file or empty directory at the given path.
    pub fn delete(&mut self, path: &str) -> Result<()> {
        let (parent_anode, name) = self.split_path(path)?;
        self.delete_in(parent_anode, &name)
    }

    /// Set the volume name (max 30 characters).
    pub fn set_volume_name(&mut self, name: &str) -> Result<()> {
        let name_bytes = name.as_bytes();
        let len = name_bytes.len().min(30);
        self.vol.rootblock.diskname = name[..len].to_string();
        // Patch the rootblock cluster in-place
        let bs = self.vol.block_size() as usize;
        let rblkcluster = self.vol.rootblock.rblkcluster as u32;
        let cluster_size = rblkcluster as usize * bs;
        let mut cluster = vec![0u8; cluster_size];
        self.vol
            .dev
            .read_blocks(self.firstreserved as u64, rblkcluster, &mut cluster)?;
        for b in &mut cluster[RB_OFF_DISKNAME..RB_OFF_DISKNAME + 32] {
            *b = 0;
        }
        cluster[RB_OFF_DISKNAME] = len as u8;
        cluster[RB_OFF_DISKNAME + 1..RB_OFF_DISKNAME + 1 + len].copy_from_slice(&name_bytes[..len]);
        let ds = self.next_datestamp();
        put_u32(&mut cluster, RB_OFF_DATESTAMP, ds);
        self.vol
            .dev
            .write_blocks(self.firstreserved as u64, rblkcluster, &cluster)?;
        self.vol.dev.flush()
    }

    // ---- Anode-based API (for FUSE) ----

    /// Create a file in a directory identified by anode.
    pub fn write_file_in(&mut self, parent_anode: u32, name: &str, data: &[u8]) -> Result<()> {
        self.write_file_in_no_commit(parent_anode, name, data)?;
        self.update_rootblock()
    }

    /// Write file without committing — caller must call update_rootblock().
    fn write_file_in_no_commit(
        &mut self,
        parent_anode: u32,
        name: &str,
        data: &[u8],
    ) -> Result<()> {
        let bs = self.vol.block_size() as usize;
        let num_blocks = data.len().div_ceil(bs).max(1);

        let blocks = self.alloc_data_blocks(num_blocks as u32)?;
        for (i, &blk) in blocks.iter().enumerate() {
            let start = i * bs;
            let end = (start + bs).min(data.len());
            let mut sector = vec![0u8; bs];
            if start < data.len() {
                sector[..end - start].copy_from_slice(&data[start..end]);
            }
            self.vol.dev.write_block(blk as u64, &sector)?;
        }
        self.vol.dev.flush()?; // data durable before metadata

        let anodenr = self.create_anode_chain(&blocks)?;
        self.add_dir_entry(parent_anode, name, ST_FILE, anodenr, data.len() as u64, 0)
    }

    /// Create a directory in a parent identified by anode. Returns the new dir's anode number.
    pub fn create_dir_in(&mut self, parent_anode: u32, name: &str) -> Result<()> {
        let dir_blk = self.alloc_reserved_block()?;
        let anodenr = self.alloc_anode(1, dir_blk, 0)?;

        let mut dir_data = vec![0u8; self.resblocksize as usize];
        put_u16(&mut dir_data, 0x00, DBLKID);
        put_u32(&mut dir_data, 0x04, self.next_datestamp());
        put_u32(&mut dir_data, 0x0C, anodenr);
        put_u32(&mut dir_data, 0x10, parent_anode);
        self.write_reserved(dir_blk, &dir_data)?;

        self.add_dir_entry(parent_anode, name, ST_USERDIR, anodenr, 0, 0)?;
        self.update_rootblock()
    }

    /// Create a softlink in a parent directory.
    pub fn create_softlink(&mut self, path: &str, target: &str) -> Result<()> {
        let (parent_anode, name) = self.split_path(path)?;
        self.create_softlink_in(parent_anode, &name, target)
    }

    /// Create a softlink by parent anode.
    pub fn create_softlink_in(
        &mut self,
        parent_anode: u32,
        name: &str,
        target: &str,
    ) -> Result<()> {
        let data = target.as_bytes();
        let bs = self.vol.block_size() as usize;
        let num_blocks = data.len().div_ceil(bs).max(1);
        let blocks = self.alloc_data_blocks(num_blocks as u32)?;
        for (i, &blk) in blocks.iter().enumerate() {
            let start = i * bs;
            let end = (start + bs).min(data.len());
            let mut sector = vec![0u8; bs];
            if start < data.len() {
                sector[..end - start].copy_from_slice(&data[start..end]);
            }
            self.vol.dev.write_block(blk as u64, &sector)?;
        }
        self.vol.dev.flush()?; // data durable before metadata
        let anodenr = self.create_anode_chain(&blocks)?;
        self.add_dir_entry(
            parent_anode,
            name,
            ST_SOFTLINK,
            anodenr,
            data.len() as u64,
            0,
        )?;
        self.update_rootblock()
    }

    /// Create a hardlink in a parent directory.
    pub fn create_hardlink(&mut self, path: &str, target_anode: u32) -> Result<()> {
        let (parent_anode, name) = self.split_path(path)?;
        self.add_dir_entry(parent_anode, &name, ST_LINKFILE, target_anode, 0, 0)?;
        self.update_rootblock()
    }

    /// Undelete a file from the deldir by index. Writes it to `dest_path`.
    pub fn undelete(&mut self, deldir_idx: usize, dest_path: &str) -> Result<()> {
        // Read the deldir entry
        let rext = self
            .vol
            .rootblock_ext
            .as_ref()
            .ok_or_else(|| Error::NotFound("no rootblock extension".into()))?;
        let deldirblocks: Vec<u32> = rext
            .deldirblocks
            .iter()
            .copied()
            .filter(|&b| b != 0)
            .collect();
        let rbs = self.vol.rootblock.reserved_blksize;
        let entries_per_block = deldir_entries_per_block(rbs);
        if entries_per_block == 0 {
            return Err(Error::Corrupt("invalid reserved block size".into()));
        }

        let block_idx = deldir_idx / entries_per_block;
        let slot_idx = deldir_idx % entries_per_block;
        if block_idx >= deldirblocks.len() {
            return Err(Error::NotFound(format!(
                "deldir index {} out of range",
                deldir_idx
            )));
        }
        let blk = deldirblocks[block_idx];
        let data = self.read_reserved_raw(blk)?;
        let off = DELDIR_HEADER_SIZE + slot_idx * DELDIR_ENTRY_SIZE;
        let entry = DelDirEntry::parse(&data[off..off + DELDIR_ENTRY_SIZE])
            .ok_or_else(|| Error::NotFound("empty deldir slot".into()))?;

        // Check destination doesn't already exist
        if self.vol.lookup(dest_path)?.is_some() {
            return Err(Error::AlreadyExists(dest_path.to_string()));
        }

        let old_anode = entry.anode;

        // Read file data via the anode chain (still intact)
        let file_data = self.vol.read_file_data(old_anode, entry.file_size())?;

        // Write as new file (no commit yet)
        let (parent_anode, filename) = self.split_path(dest_path)?;
        self.write_file_in_no_commit(parent_anode, &filename, &file_data)?;

        // Free old anode chain
        let _ = self.clear_anode_chain(old_anode);

        // Clear the deldir slot
        let mut block_data = self.read_reserved_raw(blk)?;
        for b in &mut block_data[off..off + DELDIR_ENTRY_SIZE] {
            *b = 0;
        }
        self.write_reserved(blk, &block_data)?;

        // Single commit
        self.update_rootblock()
    }

    /// Force-remove a directory entry without touching anodes or data blocks.
    /// Used by check --repair for entries with broken anode chains.
    pub fn force_remove_entry(&mut self, parent_anode: u32, name: &str) -> Result<()> {
        self.remove_dir_entry(parent_anode, name)?;
        self.update_rootblock()
    }

    /// Repair: set the rootblock's blocksfree field.
    pub fn repair_blocksfree(&mut self, correct_free: u32) -> Result<()> {
        self.vol.rootblock.blocksfree = correct_free;
        self.update_rootblock()
    }

    /// Repair: set the rootblock's reserved_free field.
    pub fn repair_reserved_free(&mut self, correct_free: u32) -> Result<()> {
        self.vol.rootblock.reserved_free = correct_free;
        self.update_rootblock()
    }

    /// Overwrite an existing file's data in-place, reusing its anode.
    /// The anode number stays stable — safe for FUSE inode caching.
    pub fn overwrite_file_in(
        &mut self,
        parent_anode: u32,
        name: &str,
        file_anode: u32,
        data: &[u8],
    ) -> Result<()> {
        let bs = self.vol.block_size() as usize;
        let new_blocks_needed = data.len().div_ceil(bs).max(1) as u32;

        // Get existing chain
        let old_chain =
            self.vol
                .anodes
                .get_chain(file_anode, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        let old_total: u32 = old_chain.iter().map(|a| a.clustersize).sum();

        // Write data to existing blocks (reuse as many as possible)
        let mut written = 0usize;
        let mut blocks_used = 0u32;
        let mut sector = vec![0u8; bs];
        for an in &old_chain {
            for i in 0..an.clustersize {
                if blocks_used >= new_blocks_needed {
                    break;
                }
                sector.fill(0);
                let start = written;
                let end = (start + bs).min(data.len());
                if start < data.len() {
                    sector[..end - start].copy_from_slice(&data[start..end]);
                }
                self.vol
                    .dev
                    .write_block(an.blocknr as u64 + i as u64, &sector)?;
                written += bs;
                blocks_used += 1;
            }
            if blocks_used >= new_blocks_needed {
                break;
            }
        }
        self.vol.dev.flush()?;

        if new_blocks_needed <= old_total {
            // Shrink: free excess blocks and truncate the anode chain
            self.truncate_anode_chain(file_anode, new_blocks_needed)?;
        } else {
            // Grow: allocate additional blocks and extend the chain
            let extra = new_blocks_needed - old_total;
            let new_blocks = self.alloc_data_blocks(extra)?;
            for &blk in &new_blocks {
                sector.fill(0);
                let start = written;
                let end = (start + bs).min(data.len());
                if start < data.len() {
                    sector[..end - start].copy_from_slice(&data[start..end]);
                }
                self.vol.dev.write_block(blk as u64, &sector)?;
                written += bs;
            }
            self.vol.dev.flush()?;
            // Extend the existing chain with new blocks
            let new_chain_head = self.create_anode_chain(&new_blocks)?;
            self.append_to_anode_chain(file_anode, new_chain_head)?;
        }

        // Update file size in the directory entry
        self.update_dir_entry_size(parent_anode, name, data.len() as u64)?;
        self.update_rootblock()
    }

    /// Truncate an anode chain to `keep_blocks` total blocks.
    /// Frees excess data blocks and anode slots.
    fn truncate_anode_chain(&mut self, head: u32, keep_blocks: u32) -> Result<()> {
        let chain = self
            .vol
            .anodes
            .get_chain(head, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        let mut remaining = keep_blocks;

        for (idx, an) in chain.iter().enumerate() {
            if remaining == 0 {
                self.free_and_clear_anodes(&chain[idx..])?;
                return Ok(());
            } else if remaining < an.clustersize {
                // Partial: free tail blocks, shrink clustersize, set next=EOF
                for i in remaining..an.clustersize {
                    self.free_data_block(an.blocknr + i)?;
                }
                self.write_anode_fields(an.nr, remaining, an.blocknr, ANODE_EOF)?;
                self.free_and_clear_anodes(&chain[idx + 1..])?;
                return Ok(());
            } else {
                remaining -= an.clustersize;
                if remaining == 0 {
                    // This anode is the new tail — set next=EOF
                    self.write_anode_fields(an.nr, an.clustersize, an.blocknr, ANODE_EOF)?;
                    self.free_and_clear_anodes(&chain[idx + 1..])?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Free all data blocks and clear anode slots for a slice of anodes.
    fn free_and_clear_anodes(&mut self, anodes: &[crate::ondisk::Anode]) -> Result<()> {
        for an in anodes {
            for i in 0..an.clustersize {
                self.free_data_block(an.blocknr + i)?;
            }
            self.clear_single_anode(an.nr)?;
        }
        Ok(())
    }

    /// Append a sub-chain to the tail of an existing anode chain.
    fn append_to_anode_chain(&mut self, head: u32, new_head: u32) -> Result<()> {
        let chain = self
            .vol
            .anodes
            .get_chain(head, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        let tail = chain.last().ok_or(Error::AnodeNotFound(head))?;
        self.write_anode_fields(tail.nr, tail.clustersize, tail.blocknr, new_head)
    }

    /// Write the 3 fields of a single anode slot on disk.
    fn write_anode_fields(
        &mut self,
        anodenr: u32,
        clustersize: u32,
        blocknr: u32,
        next: u32,
    ) -> Result<()> {
        let split = self.vol.rootblock.is_splitted_anodes();
        let (seqnr, offset) = if split {
            (anodenr >> 16, anodenr & 0xFFFF)
        } else {
            (
                anodenr / self.anodes_per_block,
                anodenr % self.anodes_per_block,
            )
        };
        let blk_num = self.get_anode_block_nr(seqnr)?;
        let mut data = self.read_reserved_raw(blk_num)?;
        let base = ANODE_BLOCK_HEADER_SIZE + offset as usize * ANODE_SIZE;
        put_u32(&mut data, base, clustersize);
        put_u32(&mut data, base + 4, blocknr);
        put_u32(&mut data, base + 8, next);
        put_u32(&mut data, 4, self.datestamp);
        self.write_reserved(blk_num, &data)
    }

    /// Clear a single anode slot (set all 3 fields to 0).
    fn clear_single_anode(&mut self, anodenr: u32) -> Result<()> {
        self.write_anode_fields(anodenr, 0, 0, 0)
    }

    /// Update the fsize (and fsizex) of an existing directory entry in-place.
    fn update_dir_entry_size(&mut self, dir_anode: u32, name: &str, new_size: u64) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(dir_anode, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            for i in 0..an.clustersize {
                let blk = an.blocknr + i;
                let mut data = self.read_reserved_raw(blk)?;
                if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DBLKID {
                    continue;
                }
                let mut pos = DIR_BLOCK_HEADER_SIZE;
                while pos < self.resblocksize as usize {
                    let esize = data[pos] as usize;
                    if esize == 0 {
                        break;
                    }
                    let nlen = data[pos + 17] as usize;
                    let ename = crate::util::latin1_to_string(&data[pos + 18..pos + 18 + nlen]);
                    if crate::util::name_eq_ci(&ename, name) {
                        // Patch fsize (low 32 bits)
                        put_u32(&mut data, pos + 6, new_size as u32);
                        // Walk extra fields to patch fsizex if present
                        let coff = pos + 18 + nlen;
                        if coff < pos + esize {
                            let clen = data[coff] as usize;
                            let mut fp = coff + 1 + clen;
                            if fp & 1 != 0 {
                                fp += 1;
                            }
                            let end = pos + esize;
                            if fp + 2 <= end {
                                let flags =
                                    u16::from_be_bytes(data[fp..fp + 2].try_into().unwrap());
                                fp += 2;
                                // Skip fields in order; bail via block if truncated
                                'extra: {
                                    if flags & 0x0001 != 0 {
                                        fp += 4;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0002 != 0 {
                                        fp += 2;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0004 != 0 {
                                        fp += 2;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0008 != 0 {
                                        fp += 4;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0010 != 0 {
                                        fp += 4;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0020 != 0 {
                                        fp += 4;
                                        if fp > end {
                                            break 'extra;
                                        }
                                    }
                                    if flags & 0x0040 != 0 && fp + 2 <= end {
                                        put_u16(&mut data, fp, (new_size >> 32) as u16);
                                    }
                                }
                            }
                        }
                        // Update datestamp
                        let (cday, cmin, ctick) = crate::util::current_amiga_datestamp();
                        put_u16(&mut data, pos + 10, cday);
                        put_u16(&mut data, pos + 12, cmin);
                        put_u16(&mut data, pos + 14, ctick);
                        put_u32(&mut data, 4, self.next_datestamp());
                        self.write_reserved(blk, &data)?;
                        return Ok(());
                    }
                    pos += esize;
                }
            }
        }
        Err(Error::NotFound(name.to_string()))
    }

    /// Update the protection bits of an existing directory entry in-place.
    pub fn update_dir_entry_protection(
        &mut self,
        dir_anode: u32,
        name: &str,
        protection: u8,
    ) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(dir_anode, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            for i in 0..an.clustersize {
                let blk = an.blocknr + i;
                let mut data = self.read_reserved_raw(blk)?;
                if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DBLKID {
                    continue;
                }
                let mut pos = DIR_BLOCK_HEADER_SIZE;
                while pos < self.resblocksize as usize {
                    let esize = data[pos] as usize;
                    if esize == 0 {
                        break;
                    }
                    let nlen = data[pos + 17] as usize;
                    let ename = crate::util::latin1_to_string(&data[pos + 18..pos + 18 + nlen]);
                    if crate::util::name_eq_ci(&ename, name) {
                        data[pos + 16] = protection;
                        put_u32(&mut data, 4, self.next_datestamp());
                        self.write_reserved(blk, &data)?;
                        return self.update_rootblock();
                    }
                    pos += esize;
                }
            }
        }
        Err(Error::NotFound(name.to_string()))
    }

    pub fn rename_in(
        &mut self,
        src_parent: u32,
        src_name: &str,
        dst_parent: u32,
        dst_name: &str,
    ) -> Result<()> {
        let entries = self.vol.list_dir_by_anode(src_parent)?;
        let entry = entries
            .iter()
            .find(|e| crate::util::name_eq_ci(&e.name, src_name))
            .ok_or_else(|| Error::NotFound(src_name.to_string()))?
            .clone();

        // If destination exists, delete it first
        if let Ok(dst_entries) = self.vol.list_dir_by_anode(dst_parent)
            && dst_entries
                .iter()
                .any(|e| crate::util::name_eq_ci(&e.name, dst_name))
        {
            self.delete_in(dst_parent, dst_name)?;
        }

        // Add entry in new location with new name
        self.add_dir_entry(
            dst_parent,
            dst_name,
            entry.entry_type,
            entry.anode,
            entry.file_size(),
            entry.protection,
        )?;
        // Remove from old location
        self.remove_dir_entry(src_parent, src_name)?;
        self.update_rootblock()
    }

    /// Delete a file or empty directory by name in a parent directory.
    pub fn delete_in(&mut self, parent_anode: u32, name: &str) -> Result<()> {
        let entries = self.vol.list_dir_by_anode(parent_anode)?;
        let target = entries
            .iter()
            .find(|e| crate::util::name_eq_ci(&e.name, name))
            .ok_or_else(|| Error::NotFound(name.to_string()))?
            .clone();

        if target.is_dir() {
            let sub = self.vol.list_dir_by_anode(target.anode)?;
            if !sub.is_empty() {
                return Err(Error::NotEmpty);
            }
            self.free_anode_chain_reserved(target.anode)?;
            self.clear_anode_chain(target.anode)?;
        } else {
            // Try to move to deldir instead of freeing
            if !self.move_to_deldir(&target) {
                self.free_data_blocks(target.anode)?;
                self.clear_anode_chain(target.anode)?;
            }
        }
        self.remove_dir_entry(parent_anode, name)?;
        self.update_rootblock()
    }

    /// Move a deleted file entry to the deldir. Returns false if deldir not enabled.
    fn move_to_deldir(&mut self, entry: &crate::ondisk::DirEntry) -> bool {
        use crate::ondisk::*;
        if !self.vol.rootblock.has_flag(MODE_DELDIR) {
            return false;
        }
        let rext = match &self.vol.rootblock_ext {
            Some(e) => e,
            None => return false,
        };
        let deldirblocks: Vec<u32> = rext
            .deldirblocks
            .iter()
            .copied()
            .filter(|&b| b != 0)
            .collect();
        if deldirblocks.is_empty() {
            return false;
        }

        let rbs = self.vol.rootblock.reserved_blksize;
        let entries_per_block = deldir_entries_per_block(rbs);

        // Find a free slot (anode == 0) using roving pointer
        for blk in &deldirblocks {
            let data = match self.read_reserved_raw(*blk) {
                Ok(d) => d,
                Err(_) => continue,
            };
            if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DELDIRID {
                continue;
            }

            for i in 0..entries_per_block {
                let off = DELDIR_HEADER_SIZE + i * DELDIR_ENTRY_SIZE;
                if off + DELDIR_ENTRY_SIZE > data.len() {
                    break;
                }
                let slot_anode = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
                if slot_anode == 0 {
                    // Found free slot — write the deldir entry
                    let mut block_data = data;
                    self.write_deldir_entry(&mut block_data, off, entry);
                    let _ = self.write_reserved(*blk, &block_data);
                    return true;
                }
            }
        }

        // Deldir full — evict oldest entry (first slot of first block)
        let blk = deldirblocks[0];
        let data = match self.read_reserved_raw(blk) {
            Ok(d) => d,
            Err(_) => return false,
        };
        let off = DELDIR_HEADER_SIZE;
        let evict_anode = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
        if evict_anode != 0 {
            let _ = self.free_data_blocks(evict_anode);
            let _ = self.clear_anode_chain(evict_anode);
        }
        let mut block_data = data;
        self.write_deldir_entry(&mut block_data, off, entry);
        let _ = self.write_reserved(blk, &block_data);
        true
    }

    fn write_deldir_entry(&self, block: &mut [u8], off: usize, entry: &crate::ondisk::DirEntry) {
        put_u32(block, off, entry.anode);
        put_u32(block, off + 4, entry.file_size() as u32);
        put_u16(block, off + 8, entry.creation_day);
        put_u16(block, off + 10, entry.creation_minute);
        put_u16(block, off + 12, entry.creation_tick);
        let name_bytes = entry.name.as_bytes();
        let len = name_bytes.len().min(16);
        for b in &mut block[off + 14..off + 30] {
            *b = 0;
        }
        block[off + 14..off + 14 + len].copy_from_slice(&name_bytes[..len]);
        put_u16(block, off + 30, (entry.file_size() >> 32) as u16);
    }

    // ---- Data bitmap ----

    fn load_data_bitmap(&mut self) -> Result<()> {
        let no_bmb = {
            let bits_per_bmb = self.index_per_block * 32;
            let ds = self.vol.rootblock.disksize;
            ds.div_ceil(bits_per_bmb)
        };
        for seq in 0..no_bmb {
            if let Some(blk) = self.get_bitmap_block_nr(seq)? {
                let data = self.read_reserved_raw(blk)?;
                let mut longs = Vec::new();
                for i in 0..self.index_per_block as usize {
                    let off = 12 + i * 4;
                    if off + 4 <= data.len() {
                        longs.push(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()));
                    }
                }
                self.data_bm.push((blk, longs));
            }
        }
        Ok(())
    }

    fn alloc_data_blocks(&mut self, count: u32) -> Result<Vec<u32>> {
        let mut allocated = Vec::new();
        for bm_idx in 0..self.data_bm.len() {
            let (_, ref mut longs) = self.data_bm[bm_idx];
            #[allow(clippy::needless_range_loop)]
            for li in 0..longs.len() {
                if longs[li] == 0 {
                    continue;
                }
                for bit in 0..32u32 {
                    if longs[li] & (0x8000_0000 >> bit) != 0 {
                        let data_blk = (bm_idx as u32)
                            .checked_mul(self.index_per_block)
                            .and_then(|v| v.checked_mul(32))
                            .and_then(|v| v.checked_add(li as u32 * 32 + bit))
                            .and_then(|v| v.checked_add(self.bitmapstart))
                            .ok_or_else(|| {
                                Error::Corrupt("block number overflow in bitmap".into())
                            })?;
                        longs[li] &= !(0x8000_0000 >> bit);
                        allocated.push(data_blk);
                        if allocated.len() as u32 == count {
                            self.write_data_bitmap_block(bm_idx)?;
                            self.vol.rootblock.blocksfree -= count;
                            return Ok(allocated);
                        }
                    }
                }
            }
            if !allocated.is_empty() {
                self.write_data_bitmap_block(bm_idx)?;
            }
        }
        Err(Error::DiskFull(format!(
            "not enough free blocks (need {})",
            count
        )))
    }

    fn write_data_bitmap_block(&mut self, bm_idx: usize) -> Result<()> {
        let (blk, ref longs) = self.data_bm[bm_idx];
        let mut data = self.read_reserved_raw(blk)?;
        put_u32(&mut data, 4, self.datestamp);
        for (i, &val) in longs.iter().enumerate() {
            put_u32(&mut data, 12 + i * 4, val);
        }
        self.write_reserved(blk, &data)
    }

    fn free_data_blocks(&mut self, anodenr: u32) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(anodenr, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            for i in 0..an.clustersize {
                self.free_data_block(an.blocknr + i)?;
            }
        }
        Ok(())
    }

    fn free_data_block(&mut self, blk: u32) -> Result<()> {
        if blk < self.bitmapstart {
            return Ok(());
        }
        let rel = blk - self.bitmapstart;
        let bm_idx = rel / (self.index_per_block * 32);
        let remainder = rel % (self.index_per_block * 32);
        let li = (remainder / 32) as usize;
        let bit = remainder % 32;
        if (bm_idx as usize) < self.data_bm.len() {
            self.data_bm[bm_idx as usize].1[li] |= 0x8000_0000 >> bit;
            self.write_data_bitmap_block(bm_idx as usize)?;
            self.vol.rootblock.blocksfree += 1;
        }
        Ok(())
    }

    // ---- Reserved bitmap ----

    fn load_reserved_bitmap(&mut self) -> Result<()> {
        let rb = &self.vol.rootblock;
        let bs = self.vol.block_size() as usize;
        let cluster_size = rb.rblkcluster as usize * bs;
        let mut cluster = vec![0u8; cluster_size];
        self.vol.dev.read_blocks(
            self.firstreserved as u64,
            rb.rblkcluster as u32,
            &mut cluster,
        )?;
        let bm_off = bs + 12; // after rootblock sector + BM header
        self.res_bitmap.clear();
        for i in 0..=(self.numreserved / 32) {
            let off = bm_off + i as usize * 4;
            if off + 4 <= cluster.len() {
                self.res_bitmap.push(u32::from_be_bytes(
                    cluster[off..off + 4].try_into().unwrap(),
                ));
            }
        }
        Ok(())
    }

    fn alloc_reserved_block(&mut self) -> Result<u32> {
        for li in 0..self.res_bitmap.len() {
            if self.res_bitmap[li] == 0 {
                continue;
            }
            for bit in 0..32u32 {
                if self.res_bitmap[li] & (0x8000_0000 >> bit) != 0 {
                    let idx = li as u32 * 32 + bit;
                    self.res_bitmap[li] &= !(0x8000_0000 >> bit);
                    self.vol.rootblock.reserved_free -= 1;
                    return Ok(self.firstreserved + idx * self.rescluster);
                }
            }
        }
        Err(Error::DiskFull("out of reserved blocks".into()))
    }

    fn free_reserved_block(&mut self, blk: u32) -> Result<()> {
        let idx = (blk - self.firstreserved) / self.rescluster;
        let li = (idx / 32) as usize;
        let bit = idx % 32;
        if li < self.res_bitmap.len() {
            self.res_bitmap[li] |= 0x8000_0000 >> bit;
            self.vol.rootblock.reserved_free += 1;
        }
        Ok(())
    }

    // ---- Anode allocation ----

    fn alloc_anode(&mut self, clustersize: u32, blocknr: u32, next: u32) -> Result<u32> {
        let split = self.vol.rootblock.is_splitted_anodes();
        for seqnr in 0..256u32 {
            let blk_num = self.get_anode_block_nr(seqnr)?;
            if blk_num == 0 {
                break;
            }
            let mut data = self.read_reserved_raw(blk_num)?;
            if u16::from_be_bytes(data[0..2].try_into().unwrap()) != ABLKID {
                continue;
            }
            for offset in 0..self.anodes_per_block {
                let anodenr = if split {
                    (seqnr << 16) | offset
                } else {
                    seqnr * self.anodes_per_block + offset
                };
                if anodenr < ANODE_USERFIRST {
                    continue;
                }
                let base = ANODE_BLOCK_HEADER_SIZE + offset as usize * ANODE_SIZE;
                let cs = u32::from_be_bytes(data[base..base + 4].try_into().unwrap());
                let bn = u32::from_be_bytes(data[base + 4..base + 8].try_into().unwrap());
                if cs == 0 && bn == 0 {
                    put_u32(&mut data, base, clustersize);
                    put_u32(&mut data, base + 4, blocknr);
                    put_u32(&mut data, base + 8, next);
                    put_u32(&mut data, 4, self.datestamp);
                    self.write_reserved(blk_num, &data)?;
                    return Ok(anodenr);
                }
            }
        }
        Err(Error::DiskFull("no free anode slots".into()))
    }

    fn create_anode_chain(&mut self, blocks: &[u32]) -> Result<u32> {
        let mut clusters = Vec::new();
        let mut i = 0usize;
        while i < blocks.len() {
            let start = blocks[i];
            let mut count = 1u32;
            while i + (count as usize) < blocks.len() && blocks[i + count as usize] == start + count
            {
                count += 1;
            }
            clusters.push((start, count));
            i += count as usize;
        }
        // Allocate in reverse so we can set next pointers
        let mut next_nr = 0u32;
        for &(start, count) in clusters.iter().rev() {
            next_nr = self.alloc_anode(count, start, next_nr)?;
        }
        Ok(next_nr)
    }

    fn clear_anode_chain(&mut self, anodenr: u32) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(anodenr, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            self.clear_single_anode(an.nr)?;
        }
        Ok(())
    }

    fn free_anode_chain_reserved(&mut self, anodenr: u32) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(anodenr, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            for i in 0..an.clustersize {
                self.free_reserved_block(an.blocknr + i)?;
            }
        }
        Ok(())
    }

    // ---- Directory entry ----

    fn add_dir_entry(
        &mut self,
        dir_anode: u32,
        name: &str,
        entry_type: i8,
        anode: u32,
        fsize: u64,
        protection: u8,
    ) -> Result<()> {
        let entry_bytes = self.build_dir_entry(name, entry_type, anode, fsize, protection);
        let chain =
            self.vol
                .anodes
                .get_chain(dir_anode, self.vol.dev.as_ref(), &mut self.vol.cache)?;

        for an in &chain {
            for i in 0..an.clustersize {
                let blk = an.blocknr + i;
                let mut data = self.read_reserved_raw(blk)?;
                if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DBLKID {
                    continue;
                }
                // Find end of entries
                let mut pos = DIR_BLOCK_HEADER_SIZE;
                while pos < self.resblocksize as usize {
                    if data[pos] == 0 {
                        break;
                    }
                    pos += data[pos] as usize;
                }
                if pos + entry_bytes.len() < self.resblocksize as usize {
                    data[pos..pos + entry_bytes.len()].copy_from_slice(&entry_bytes);
                    if pos + entry_bytes.len() < self.resblocksize as usize {
                        data[pos + entry_bytes.len()] = 0;
                    }
                    put_u32(&mut data, 4, self.next_datestamp());
                    self.write_reserved(blk, &data)?;
                    return Ok(());
                }
            }
        }
        // No space — allocate new dir block and extend chain
        let new_blk = self.alloc_reserved_block()?;
        let mut new_data = vec![0u8; self.resblocksize as usize];
        put_u16(&mut new_data, 0x00, DBLKID);
        put_u32(&mut new_data, 0x04, self.next_datestamp());
        put_u32(&mut new_data, 0x0C, dir_anode);
        put_u32(&mut new_data, 0x10, dir_anode);
        new_data[DIR_BLOCK_HEADER_SIZE..DIR_BLOCK_HEADER_SIZE + entry_bytes.len()]
            .copy_from_slice(&entry_bytes);
        self.write_reserved(new_blk, &new_data)?;
        self.extend_anode_chain(dir_anode, new_blk)
    }

    fn remove_dir_entry(&mut self, dir_anode: u32, name: &str) -> Result<()> {
        let chain =
            self.vol
                .anodes
                .get_chain(dir_anode, self.vol.dev.as_ref(), &mut self.vol.cache)?;
        for an in &chain {
            for i in 0..an.clustersize {
                let blk = an.blocknr + i;
                let mut data = self.read_reserved_raw(blk)?;
                if u16::from_be_bytes(data[0..2].try_into().unwrap()) != DBLKID {
                    continue;
                }
                let mut pos = DIR_BLOCK_HEADER_SIZE;
                while pos < self.resblocksize as usize {
                    let esize = data[pos] as usize;
                    if esize == 0 {
                        break;
                    }
                    let nlen = data[pos + 17] as usize;
                    let ename = crate::util::latin1_to_string(&data[pos + 18..pos + 18 + nlen]);
                    if crate::util::name_eq_ci(&ename, name) {
                        let end = pos + esize;
                        let remaining = self.resblocksize as usize - end;
                        data.copy_within(end..end + remaining, pos);
                        for b in &mut data[pos + remaining..pos + remaining + esize] {
                            *b = 0;
                        }
                        put_u32(&mut data, 4, self.next_datestamp());
                        self.write_reserved(blk, &data)?;
                        return Ok(());
                    }
                    pos += esize;
                }
            }
        }
        Err(Error::NotFound(name.to_string()))
    }

    fn build_dir_entry(
        &self,
        name: &str,
        entry_type: i8,
        anode: u32,
        fsize: u64,
        protection: u8,
    ) -> Vec<u8> {
        let name_bytes = name.as_bytes();
        let nlen = name_bytes.len().min(107);
        let fsizex = (fsize >> 32) as u16;
        let has_fsizex = fsizex > 0;
        let extra_bytes = if has_fsizex { 4 } else { 2 }; // flags(2) + optional fsizex(2)
        let mut base_size = 18 + nlen + 1 + extra_bytes;
        if base_size & 1 != 0 {
            base_size += 1;
        }
        let mut entry = vec![0u8; base_size];
        entry[0] = base_size as u8;
        entry[1] = entry_type as u8;
        put_u32(&mut entry, 2, anode);
        put_u32(&mut entry, 6, fsize as u32);
        let (cday, cmin, ctick) = crate::util::current_amiga_datestamp();
        put_u16(&mut entry, 10, cday);
        put_u16(&mut entry, 12, cmin);
        put_u16(&mut entry, 14, ctick);
        entry[16] = protection;
        entry[17] = nlen as u8;
        entry[18..18 + nlen].copy_from_slice(&name_bytes[..nlen]);
        entry[18 + nlen] = 0; // comment length
        let ef_off = if (18 + nlen + 1) & 1 != 0 {
            18 + nlen + 2
        } else {
            18 + nlen + 1
        };
        let flags: u16 = if has_fsizex { 0x0040 } else { 0 };
        put_u16(&mut entry, ef_off, flags);
        if has_fsizex {
            put_u16(&mut entry, ef_off + 2, fsizex);
        }
        entry
    }

    fn extend_anode_chain(&mut self, head_anode: u32, new_blk: u32) -> Result<()> {
        let new_anodenr = self.alloc_anode(1, new_blk, 0)?;
        self.append_to_anode_chain(head_anode, new_anodenr)
    }

    // ---- Rootblock update ----

    fn update_rootblock(&mut self) -> Result<()> {
        // Flush all pending reserved block writes first
        self.flush_pending()?;

        // Write rootblock cluster (rootblock + reserved bitmap) last — atomic commit
        let bs = self.vol.block_size() as usize;
        let rblkcluster = self.vol.rootblock.rblkcluster as u32;
        let cluster_size = rblkcluster as usize * bs;
        let mut cluster = vec![0u8; cluster_size];
        self.vol
            .dev
            .read_blocks(self.firstreserved as u64, rblkcluster, &mut cluster)?;

        // Update rootblock fields
        let ds = self.next_datestamp();
        put_u32(&mut cluster, RB_OFF_DATESTAMP, ds);
        put_u32(
            &mut cluster,
            RB_OFF_RESERVED_FREE,
            self.vol.rootblock.reserved_free,
        );
        put_u32(
            &mut cluster,
            RB_OFF_BLOCKSFREE,
            self.vol.rootblock.blocksfree,
        );

        // Update reserved bitmap in the same cluster
        let bm_off = bs + 12;
        for (i, &val) in self.res_bitmap.iter().enumerate() {
            let off = bm_off + i * 4;
            if off + 4 <= cluster.len() {
                put_u32(&mut cluster, off, val);
            }
        }

        self.vol
            .dev
            .write_blocks(self.firstreserved as u64, rblkcluster, &cluster)?;
        self.vol.dev.flush()
    }

    /// Flush all pending reserved block writes to disk.
    fn flush_pending(&mut self) -> Result<()> {
        let bs = self.vol.block_size() as usize;
        let writes: Vec<(u32, Vec<u8>)> = self.pending_writes.drain(..).collect();
        for (blk, data) in &writes {
            write_reserved_blocks(
                self.vol.dev.as_ref(),
                *blk as u64,
                data,
                self.rescluster,
                bs,
            )?;
            self.vol.cache.invalidate(*blk as u64);
        }
        self.vol.dev.flush()
    }

    // ---- Helpers ----

    fn read_reserved_raw(&self, blk: u32) -> Result<Vec<u8>> {
        // Check pending writes first (most recent write wins)
        for (b, data) in self.pending_writes.iter().rev() {
            if *b == blk {
                return Ok(data.clone());
            }
        }
        let mut data = vec![0u8; self.resblocksize as usize];
        self.vol
            .dev
            .read_blocks(blk as u64, self.rescluster, &mut data)?;
        Ok(data)
    }

    fn write_reserved(&mut self, blk: u32, data: &[u8]) -> Result<()> {
        // Deduplicate: replace existing entry for same block
        if let Some(pos) = self.pending_writes.iter().position(|(b, _)| *b == blk) {
            self.pending_writes[pos].1 = data.to_vec();
        } else {
            self.pending_writes.push((blk, data.to_vec()));
        }
        self.vol.cache.invalidate(blk as u64);
        Ok(())
    }

    fn get_anode_block_nr(&mut self, seqnr: u32) -> Result<u32> {
        self.vol
            .anodes
            .resolve_anode_block(seqnr, self.vol.dev.as_ref(), &mut self.vol.cache)
    }

    fn get_bitmap_block_nr(&mut self, seqnr: u32) -> Result<Option<u32>> {
        self.vol
            .bitmap
            .get_bitmap_block(seqnr, self.vol.dev.as_ref(), &mut self.vol.cache)
    }

    fn split_path(&mut self, path: &str) -> Result<(u32, String)> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err(Error::NotFound("empty path".into()));
        }
        let filename = parts.last().unwrap().to_string();
        let mut parent = ANODE_ROOTDIR;
        for &part in &parts[..parts.len() - 1] {
            let entries = self.vol.list_dir_by_anode(parent)?;
            let dir = entries
                .iter()
                .find(|e| crate::util::name_eq_ci(&e.name, part) && e.is_dir())
                .ok_or_else(|| Error::NotFound(part.to_string()))?;
            parent = dir.anode;
        }
        Ok((parent, filename))
    }
}
