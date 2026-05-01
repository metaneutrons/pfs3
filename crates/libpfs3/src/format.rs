//! PFS3 filesystem formatter (mkfs).
//!
//! Creates a new PFS3 filesystem on a block device.
//! Ported from pfs3aio/format.c and amitools PFSFormat.py.
//!
//! Format sequence:
//! 1. Write boot block (PFS\1 magic)
//! 2. Calculate reserved area size
//! 3. Build rootblock + reserved bitmap
//! 4. Allocate and write rootblock extension
//! 5. Allocate and write bitmap index + bitmap blocks
//! 6. Allocate and write anode index + anode block (with ANODE_ROOTDIR)
//! 7. Write root directory block (empty)

use crate::error::{Error, Result};
use crate::io::BlockDevice;
use crate::ondisk::*;
use crate::util::current_amiga_datestamp;

/// Options for formatting a new PFS3 volume.
pub struct FormatOptions {
    pub volume_name: String,
    pub enable_deldir: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            volume_name: "Untitled".into(),
            enable_deldir: false,
        }
    }
}

/// Result of a successful format operation.
#[derive(Debug)]
pub struct FormatResult {
    pub volume_name: String,
    pub total_blocks: u64,
    pub data_blocks: u64,
    pub blocks_free: u64,
    pub num_reserved: u32,
    pub reserved_blksize: u32,
}

/// Format a block device as PFS3 with explicit size.
pub fn format_with_size(
    dev: &dyn BlockDevice,
    total_blocks: u64,
    opts: &FormatOptions,
) -> Result<FormatResult> {
    let bs = dev.block_size() as usize;

    // Determine reserved block size
    let mut resblocksize: u32 = 1024;
    let mut supermode = false;
    if total_blocks > MAXSMALLDISK {
        supermode = true;
        let max_1k = (MAXBITMAPINDEX as u64 + 1) * 253 * 253 * 32;
        let max_2k = (MAXBITMAPINDEX as u64 + 1) * 509 * 509 * 32;
        if total_blocks > max_1k {
            resblocksize = 2048;
        }
        if total_blocks > max_2k {
            resblocksize = 4096;
        }
    }
    if resblocksize < bs as u32 {
        resblocksize = bs as u32;
    }
    let rescluster = resblocksize / bs as u32; // sectors per reserved block

    // Calculate number of reserved blocks
    let numreserved = calc_num_reserved(total_blocks, resblocksize);

    // Reserved bitmap size (in reserved blocks)
    let mut resbm_1k = 1u32;
    let mut i = 125u32;
    while i < numreserved / 32 {
        resbm_1k += 1;
        i += 256;
    }
    let resbm_resblocks = (1024 * resbm_1k).div_ceil(resblocksize);
    let rblkcluster = rescluster * resbm_resblocks;

    let firstreserved: u32 = 2;
    let lastreserved = rescluster * numreserved + firstreserved - 1;

    // Reserved block allocator
    let mut alloc = ReservedAllocator::new(numreserved, resbm_resblocks);

    // Allocate rootblock extension
    let rext_idx = alloc.alloc()?;
    let rext_blk = firstreserved + rext_idx * rescluster;

    // Options
    let mut options: u32 = MODE_HARDDISK
        | MODE_SPLITTED_ANODES
        | MODE_DIR_EXTENSION
        | MODE_SIZEFIELD
        | MODE_DATESTAMP
        | MODE_EXTROVING
        | MODE_LONGFN
        | MODE_EXTENSION;
    if supermode {
        options |= MODE_SUPERINDEX;
    }

    // Timestamp (current time as Amiga datestamp)
    let (cday, cmin, ctick) = current_amiga_datestamp();

    // Index geometry — same formula as Rootblock::index_per_block()
    let index_per_block = (resblocksize / 4).saturating_sub(3);

    // Data blocks
    let reserved_area = (lastreserved - firstreserved + 1) + firstreserved;
    if total_blocks as u32 <= reserved_area {
        return Err(Error::DiskFull(format!(
            "disk too small: {} blocks, need at least {} for reserved area",
            total_blocks,
            reserved_area + 1
        )));
    }
    let data_blocks = total_blocks as u32 - reserved_area;
    let bits_per_bmb = index_per_block * 32;
    let no_bmb = data_blocks.div_ceil(bits_per_bmb);
    let no_bmi = no_bmb.div_ceil(index_per_block);

    // 1. Boot block
    let mut boot = vec![0u8; bs];
    boot[0..4].copy_from_slice(&ID_PFS_DISK.to_be_bytes());
    dev.write_block(0, &boot)?;
    dev.write_block(1, &vec![0u8; bs])?;

    // 2. Allocate bitmap blocks
    let mut bm_blocknrs = Vec::new();
    for _ in 0..no_bmb {
        let idx = alloc.alloc()?;
        bm_blocknrs.push(firstreserved + idx * rescluster);
    }

    // 3. Allocate bitmap index blocks
    let mut bmi_blocknrs = Vec::new();
    for _ in 0..no_bmi {
        let idx = alloc.alloc()?;
        bmi_blocknrs.push(firstreserved + idx * rescluster);
    }

    // 4. Allocate anode index + anode block
    let anidx_blk = firstreserved + alloc.alloc()? * rescluster;
    let anode_blk = firstreserved + alloc.alloc()? * rescluster;

    // 5. Allocate root directory block
    let rootdir_blk = firstreserved + alloc.alloc()? * rescluster;

    // Build rootblock + reserved bitmap
    let rb_size = rblkcluster as usize * bs;
    let mut rb_data = vec![0u8; rb_size];

    // Rootblock fields
    put_u32(&mut rb_data, 0x00, ID_PFS_DISK);
    put_u32(&mut rb_data, 0x04, options);
    put_u32(&mut rb_data, 0x08, 1); // datestamp
    put_u16(&mut rb_data, 0x0C, cday);
    put_u16(&mut rb_data, 0x0E, cmin);
    put_u16(&mut rb_data, 0x10, ctick);
    put_u16(&mut rb_data, 0x12, 0xF0); // protection

    // Disk name (pascal string)
    let name = opts.volume_name.as_bytes();
    let namelen = name.len().min(30);
    rb_data[0x14] = namelen as u8;
    rb_data[0x15..0x15 + namelen].copy_from_slice(&name[..namelen]);

    put_u32(&mut rb_data, 0x34, lastreserved);
    put_u32(&mut rb_data, 0x38, firstreserved);
    put_u32(&mut rb_data, 0x3C, alloc.free_count());
    put_u16(&mut rb_data, 0x40, resblocksize as u16);
    put_u16(&mut rb_data, 0x42, rblkcluster as u16);
    put_u32(&mut rb_data, 0x44, data_blocks);
    put_u32(&mut rb_data, 0x48, data_blocks / 20); // alwaysfree
    put_u32(&mut rb_data, 0x54, total_blocks as u32); // disksize
    put_u32(&mut rb_data, 0x58, rext_blk); // extension

    // Index union at 0x60
    if supermode {
        for (i, &blknr) in bmi_blocknrs.iter().enumerate() {
            put_u32(&mut rb_data, 0x60 + i * 4, blknr);
        }
    } else {
        for (i, &blknr) in bmi_blocknrs.iter().enumerate() {
            if i <= MAXSMALLBITMAPINDEX {
                put_u32(&mut rb_data, 0x60 + i * 4, blknr);
            }
        }
        let idx_base = 0x60 + (MAXSMALLBITMAPINDEX + 1) * 4;
        put_u32(&mut rb_data, idx_base, anidx_blk);
    }

    // Reserved bitmap (after rootblock in the cluster)
    let rbm_off = bs;
    put_u16(&mut rb_data, rbm_off, BMBLKID);
    put_u32(&mut rb_data, rbm_off + 8, 0); // seqnr
    let bm_longs = alloc.build_bitmap();
    for (i, &val) in bm_longs.iter().enumerate() {
        let off = rbm_off + 12 + i * 4;
        if off + 4 <= rb_data.len() {
            put_u32(&mut rb_data, off, val);
        }
    }

    // Write rootblock cluster
    for i in 0..rblkcluster as usize {
        dev.write_block(
            (firstreserved + i as u32) as u64,
            &rb_data[i * bs..(i + 1) * bs],
        )?;
    }

    // Write rootblock extension
    let mut rext = vec![0u8; resblocksize as usize];
    put_u16(&mut rext, 0x00, EXTENSIONID);
    put_u32(&mut rext, 0x08, 1); // datestamp
    put_u32(&mut rext, 0x0C, 0x0013_0002); // pfs2version 19.2
    put_u16(&mut rext, 0x10, cday);
    put_u16(&mut rext, 0x12, cmin);
    put_u16(&mut rext, 0x14, ctick);
    put_u16(&mut rext, 0x38, 32); // fnsize
    if supermode {
        put_u32(&mut rext, 0x40, anidx_blk); // superindex[0]
    }
    write_reserved_blocks(dev, rext_blk as u64, &rext, rescluster, bs)?;

    // Write bitmap index blocks
    for (seq, &bmi_blknr) in bmi_blocknrs.iter().enumerate() {
        let mut bmi = vec![0u8; resblocksize as usize];
        put_u16(&mut bmi, 0, BMIBLKID);
        put_u32(&mut bmi, 4, 1); // datestamp
        put_u32(&mut bmi, 8, seq as u32); // seqnr
        for j in 0..index_per_block as usize {
            let bm_idx = seq * index_per_block as usize + j;
            if bm_idx < bm_blocknrs.len() {
                put_u32(&mut bmi, 12 + j * 4, bm_blocknrs[bm_idx]);
            }
        }
        write_reserved_blocks(dev, bmi_blknr as u64, &bmi, rescluster, bs)?;
    }

    // Write bitmap blocks (all data blocks free)
    for (seq, &bm_blknr) in bm_blocknrs.iter().enumerate() {
        let mut bm = vec![0u8; resblocksize as usize];
        put_u16(&mut bm, 0, BMBLKID);
        put_u32(&mut bm, 4, 1); // datestamp
        put_u32(&mut bm, 8, seq as u32); // seqnr
        for j in 0..index_per_block as usize {
            let block_idx = seq as u32 * bits_per_bmb + j as u32 * 32;
            if block_idx < data_blocks {
                let remaining = (data_blocks - block_idx).min(32);
                let val = if remaining >= 32 {
                    0xFFFF_FFFF
                } else {
                    (0xFFFF_FFFFu32) << (32 - remaining)
                };
                put_u32(&mut bm, 12 + j * 4, val);
            }
        }
        write_reserved_blocks(dev, bm_blknr as u64, &bm, rescluster, bs)?;
    }

    // Write anode index block
    let mut anidx = vec![0u8; resblocksize as usize];
    put_u16(&mut anidx, 0, IBLKID);
    put_u32(&mut anidx, 4, 1);
    put_u32(&mut anidx, 8, 0); // seqnr
    put_u32(&mut anidx, 12, anode_blk); // index[0] = anode block
    write_reserved_blocks(dev, anidx_blk as u64, &anidx, rescluster, bs)?;

    // Write anode block with ANODE_ROOTDIR
    let mut an = vec![0u8; resblocksize as usize];
    put_u16(&mut an, 0, ABLKID);
    put_u32(&mut an, 4, 1);
    put_u32(&mut an, 8, 0); // seqnr
    let an_off = ANODE_BLOCK_HEADER_SIZE + ANODE_ROOTDIR as usize * ANODE_SIZE;
    put_u32(&mut an, an_off, 1); // clustersize = 1
    put_u32(&mut an, an_off + 4, rootdir_blk); // blocknr
    put_u32(&mut an, an_off + 8, 0); // next = EOF
    write_reserved_blocks(dev, anode_blk as u64, &an, rescluster, bs)?;

    // Write root directory block (empty)
    let mut dir = vec![0u8; resblocksize as usize];
    put_u16(&mut dir, 0x00, DBLKID);
    put_u32(&mut dir, 0x04, 1); // datestamp
    put_u32(&mut dir, 0x0C, ANODE_ROOTDIR);
    put_u32(&mut dir, 0x10, ANODE_ROOTDIR); // parent = self
    write_reserved_blocks(dev, rootdir_blk as u64, &dir, rescluster, bs)?;

    dev.flush()?;

    Ok(FormatResult {
        volume_name: opts.volume_name.clone(),
        total_blocks,
        data_blocks: data_blocks as u64,
        blocks_free: data_blocks as u64,
        num_reserved: numreserved,
        reserved_blksize: resblocksize,
    })
}

// --- Helpers ---

fn calc_num_reserved(total_blocks: u64, resblocksize: u32) -> u32 {
    let mut taken: u32 = 32;
    let mut i: u64 = 2048;
    while i > 0 && i / 2 < total_blocks {
        let m: u32 = if i >= 512 * 2048 { 10 } else { 14 };
        taken += taken * m / 16;
        i = i.checked_shl(1).unwrap_or(0);
    }
    taken /= resblocksize / 1024;
    taken = taken.saturating_sub(1).min(MAXNUMRESERVED as u32);
    taken = (taken + 31) & !0x1F;
    taken.max(32)
}

const MAXSMALLDISK: u64 = (MAXSMALLBITMAPINDEX as u64 + 1) * 253 * 253 * 32;
const MAXNUMRESERVED: usize = 4096 + 255 * 1024 * 8;

// Re-export for backward compatibility.

// --- Reserved block allocator ---

struct ReservedAllocator {
    bitmap: Vec<bool>, // true = free
    numreserved: u32,
    roving: u32,
}

impl ReservedAllocator {
    fn new(numreserved: u32, rblkcluster_resblocks: u32) -> Self {
        let mut bitmap = vec![true; numreserved as usize];
        // Mark rootblock cluster as used
        for i in 0..rblkcluster_resblocks as usize {
            if i < bitmap.len() {
                bitmap[i] = false;
            }
        }
        Self {
            bitmap,
            numreserved,
            roving: rblkcluster_resblocks,
        }
    }

    fn alloc(&mut self) -> crate::error::Result<u32> {
        for i in 0..self.numreserved {
            let idx = (self.roving + i) % self.numreserved;
            if self.bitmap[idx as usize] {
                self.bitmap[idx as usize] = false;
                self.roving = (idx + 1) % self.numreserved;
                return Ok(idx);
            }
        }
        Err(crate::error::Error::DiskFull(
            "out of reserved blocks".into(),
        ))
    }

    fn free_count(&self) -> u32 {
        self.bitmap.iter().filter(|&&b| b).count() as u32
    }

    fn build_bitmap(&self) -> Vec<u32> {
        let mut longs = Vec::new();
        for i in (0..self.numreserved).step_by(32) {
            let mut val = 0u32;
            for bit in 0..32 {
                if (i + bit) < self.numreserved && self.bitmap[(i + bit) as usize] {
                    val |= 0x8000_0000 >> bit;
                }
            }
            longs.push(val);
        }
        longs
    }
}
