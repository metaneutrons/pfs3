//! PFS3 on-disk structures and constants.
//!
//! All structures are big-endian, 2-byte packed on disk.
//! We parse manually via `byteorder` — never transmute raw buffers.
//!
//! Reference: pfs3aio/blocks.h, pfs3aio/struct.h

mod direntry;
mod rootblock;

pub use direntry::*;
pub use rootblock::*;

use byteorder::{BigEndian, ReadBytesExt};
use std::io::Cursor;

use crate::error::{Error, Result};

// ---- PFS3 filesystem type identifiers ----

/// PFS1 filesystem identifier.
pub const ID_PFS_DISK: u32 = 0x5046_5301;
/// PFS2 filesystem identifier.
pub const ID_PFS2_DISK: u32 = 0x5046_5302;
/// AFS filesystem identifier.
pub const ID_AFS_DISK: u32 = 0x4146_5301;
/// muAF filesystem identifier.
pub const ID_MUAF_DISK: u32 = 0x6D75_4146;
/// muPF filesystem identifier.
pub const ID_MUPFS_DISK: u32 = 0x6D75_5046;

/// All recognized PFS3 disk type IDs.
pub const PFS_TYPES: &[u32] = &[
    ID_PFS_DISK,
    ID_PFS2_DISK,
    ID_AFS_DISK,
    ID_MUAF_DISK,
    ID_MUPFS_DISK,
];

// ---- Block type IDs ----

pub const DBLKID: u16 = 0x4442;
pub const ABLKID: u16 = 0x4142;
pub const IBLKID: u16 = 0x4942;
pub const BMBLKID: u16 = 0x424D;
pub const BMIBLKID: u16 = 0x4D49;
pub const DELDIRID: u16 = 0x4444;
pub const EXTENSIONID: u16 = 0x4558;
pub const SBLKID: u16 = 0x5342;

// ---- Rootblock option flags ----

pub const MODE_HARDDISK: u32 = 1;
pub const MODE_SPLITTED_ANODES: u32 = 2;
pub const MODE_DIR_EXTENSION: u32 = 4;
pub const MODE_DELDIR: u32 = 8;
pub const MODE_SIZEFIELD: u32 = 16;
pub const MODE_EXTENSION: u32 = 32;
pub const MODE_DATESTAMP: u32 = 64;
pub const MODE_SUPERINDEX: u32 = 128;
pub const MODE_SUPERDELDIR: u32 = 256;
pub const MODE_EXTROVING: u32 = 512;
pub const MODE_LONGFN: u32 = 1024;
pub const MODE_LARGEFILE: u32 = 2048;

// ---- Limits ----

pub const MAXSMALLBITMAPINDEX: usize = 4;
pub const MAXBITMAPINDEX: usize = 103;
pub const MAXSMALLINDEXNR: usize = 98;
pub const MAXSUPER: usize = 15;

/// Maximum directory nesting depth (prevents infinite loops on corrupt images).
pub const MAX_DIR_DEPTH: usize = 128;

// ---- Predefined anode numbers ----

pub const ANODE_EOF: u32 = 0;
pub const ANODE_ROOTDIR: u32 = 5;
pub const ANODE_USERFIRST: u32 = 6;

// ---- Well-known block positions ----

pub const BOOTBLOCK: u64 = 0;
pub const ROOTBLOCK: u64 = 2;

/// Standard Amiga sector size in bytes.
pub const SECTOR_SIZE: u32 = 512;

// ---- Directory entry types ----

pub const ST_FILE: i8 = -3;
pub const ST_USERDIR: i8 = 2;
pub const ST_SOFTLINK: i8 = 3;
pub const ST_LINKDIR: i8 = 4;
pub const ST_LINKFILE: i8 = -4;
pub const ST_ROLLOVERFILE: i8 = -16;

// ---- Anode (12 bytes on disk) ----

/// A single anode (extent descriptor): block range + next pointer.
#[derive(Debug, Clone)]
pub struct Anode {
    pub clustersize: u32,
    pub blocknr: u32,
    pub next: u32,
    /// The anode number (not stored on disk, set during lookup).
    pub nr: u32,
}

pub const ANODE_SIZE: usize = 12;

impl Anode {
    pub fn parse(data: &[u8], nr: u32) -> Result<Self> {
        if data.len() < ANODE_SIZE {
            return Err(Error::TooShort("anode"));
        }
        let mut c = Cursor::new(data);
        Ok(Self {
            clustersize: c.read_u32::<BigEndian>()?,
            blocknr: c.read_u32::<BigEndian>()?,
            next: c.read_u32::<BigEndian>()?,
            nr,
        })
    }

    pub fn is_eof(&self) -> bool {
        self.next == ANODE_EOF
    }
}

// ---- Anode Block header ----

/// Header of an anode block (16 bytes, then anode array).
#[derive(Debug)]
pub struct AnodeBlockHeader {
    pub id: u16,
    pub datestamp: u32,
    pub seqnr: u32,
}

impl AnodeBlockHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 16 {
            return Err(Error::TooShort("anode block header"));
        }
        let id = u16::from_be_bytes(data[0..2].try_into().unwrap());
        if id != ABLKID {
            return Err(Error::BadBlockId("anode block", ABLKID, id));
        }
        Ok(Self {
            id,
            datestamp: u32::from_be_bytes(data[4..8].try_into().unwrap()),
            seqnr: u32::from_be_bytes(data[8..12].try_into().unwrap()),
        })
    }
}

pub const ANODE_BLOCK_HEADER_SIZE: usize = 16;

// ---- Index Block header ----

/// Header of an index block (12 bytes, then u32 array).
#[derive(Debug)]
pub struct IndexBlockHeader {
    pub id: u16,
    pub datestamp: u32,
    pub seqnr: u32,
}

impl IndexBlockHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(Error::TooShort("index block header"));
        }
        Ok(Self {
            id: u16::from_be_bytes(data[0..2].try_into().unwrap()),
            datestamp: u32::from_be_bytes(data[4..8].try_into().unwrap()),
            seqnr: u32::from_be_bytes(data[8..12].try_into().unwrap()),
        })
    }
}

pub const INDEX_BLOCK_HEADER_SIZE: usize = 12;

// ---- Bitmap Block header ----

/// Header of a bitmap block (12 bytes, then bit array).
#[derive(Debug)]
pub struct BitmapBlockHeader {
    pub id: u16,
    pub datestamp: u32,
    pub seqnr: u32,
}

impl BitmapBlockHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 12 {
            return Err(Error::TooShort("bitmap block header"));
        }
        let id = u16::from_be_bytes(data[0..2].try_into().unwrap());
        if id != BMBLKID {
            return Err(Error::BadBlockId("bitmap block", BMBLKID, id));
        }
        Ok(Self {
            id,
            datestamp: u32::from_be_bytes(data[4..8].try_into().unwrap()),
            seqnr: u32::from_be_bytes(data[8..12].try_into().unwrap()),
        })
    }
}

pub const BITMAP_BLOCK_HEADER_SIZE: usize = 12;

// ---- Deldir (trash/undelete) ----

/// Deleted directory entry (fixed 32 bytes).
#[derive(Debug, Clone)]
pub struct DelDirEntry {
    pub anode: u32,
    pub fsize: u32,
    pub fsizex: u16,
    pub creation_day: u16,
    pub creation_minute: u16,
    pub creation_tick: u16,
    pub filename: String,
}

impl DelDirEntry {
    /// Parse a deldir entry from 32 bytes. Returns None if slot is empty.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 32 {
            return None;
        }
        let anode = u32::from_be_bytes(data[0..4].try_into().unwrap());
        if anode == 0 {
            return None;
        }
        Some(Self {
            anode,
            fsize: u32::from_be_bytes(data[4..8].try_into().unwrap()),
            creation_day: u16::from_be_bytes(data[8..10].try_into().unwrap()),
            creation_minute: u16::from_be_bytes(data[10..12].try_into().unwrap()),
            creation_tick: u16::from_be_bytes(data[12..14].try_into().unwrap()),
            filename: crate::util::latin1_to_string(&data[14..30])
                .trim_end_matches('\0')
                .to_string(),
            fsizex: u16::from_be_bytes(data[30..32].try_into().unwrap()),
        })
    }

    /// Full file size (combining fsize and fsizex).
    pub fn file_size(&self) -> u64 {
        self.fsize as u64 | ((self.fsizex as u64) << 32)
    }
}

/// Deldir block header size (before entries).
pub const DELDIR_HEADER_SIZE: usize = 32;
/// Size of one deldir entry.
pub const DELDIR_ENTRY_SIZE: usize = 32;

/// Number of deldir entries that fit in one reserved block.
pub fn deldir_entries_per_block(reserved_blksize: u16) -> usize {
    (reserved_blksize as usize).saturating_sub(DELDIR_HEADER_SIZE) / DELDIR_ENTRY_SIZE
}

// ---- Big-endian write helpers ----

/// Write a big-endian u32 at byte offset `off`.
pub fn put_u32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}

/// Write a big-endian u16 at byte offset `off`.
pub fn put_u16(buf: &mut [u8], off: usize, val: u16) {
    buf[off..off + 2].copy_from_slice(&val.to_be_bytes());
}

/// Write a reserved-size block (possibly spanning multiple device sectors).
pub fn write_reserved_blocks(
    dev: &dyn crate::io::BlockDevice,
    blk: u64,
    data: &[u8],
    rescluster: u32,
    sector_size: usize,
) -> crate::error::Result<()> {
    for i in 0..rescluster as usize {
        let start = i * sector_size;
        let end = (start + sector_size).min(data.len());
        let mut sector = vec![0u8; sector_size];
        if start < data.len() {
            sector[..end - start].copy_from_slice(&data[start..end]);
        }
        dev.write_block(blk + i as u64, &sector)?;
    }
    Ok(())
}
