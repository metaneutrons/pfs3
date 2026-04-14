//! Rootblock and rootblock extension parsing.

use byteorder::{BigEndian, ReadBytesExt};
use std::io::Cursor;

use super::*;
use crate::error::{Error, Result};

/// PFS3 rootblock — the filesystem superblock at partition block 2.
///
/// On-disk field byte offsets (all big-endian):
pub const RB_OFF_DISKTYPE: usize = 0x00;
pub const RB_OFF_OPTIONS: usize = 0x04;
pub const RB_OFF_DATESTAMP: usize = 0x08;
pub const RB_OFF_DISKNAME: usize = 0x14;
pub const RB_OFF_LASTRESERVED: usize = 0x34;
pub const RB_OFF_FIRSTRESERVED: usize = 0x38;
pub const RB_OFF_RESERVED_FREE: usize = 0x3C;
pub const RB_OFF_RESERVED_BLKSIZE: usize = 0x40;
pub const RB_OFF_RBLKCLUSTER: usize = 0x42;
pub const RB_OFF_BLOCKSFREE: usize = 0x44;
pub const RB_OFF_ALWAYSFREE: usize = 0x48;
pub const RB_OFF_DISKSIZE: usize = 0x54;
pub const RB_OFF_EXTENSION: usize = 0x58;
pub const RB_OFF_INDEX_UNION: usize = 0x60;

#[derive(Debug, Clone)]
pub struct Rootblock {
    pub disktype: u32,
    pub options: u32,
    pub datestamp: u32,
    pub creation_day: u16,
    pub creation_minute: u16,
    pub creation_tick: u16,
    pub protection: u16,
    pub diskname: String,
    pub lastreserved: u32,
    pub firstreserved: u32,
    pub reserved_free: u32,
    pub reserved_blksize: u16,
    pub rblkcluster: u16,
    pub blocksfree: u32,
    pub alwaysfree: u32,
    pub roving_ptr: u32,
    pub deldir: u32,
    pub disksize: u32,
    pub extension: u32,
    /// Bitmap index block pointers (small: 5, large: 104).
    pub bitmapindex: Vec<u32>,
    /// Anode index block pointers (small mode only, up to 99).
    pub indexblocks: Vec<u32>,
}

impl Rootblock {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x60 {
            return Err(Error::TooShort("rootblock"));
        }
        let mut c = Cursor::new(data);

        let disktype = c.read_u32::<BigEndian>()?;
        if !PFS_TYPES.contains(&disktype) {
            return Err(Error::BadMagic("rootblock", disktype));
        }

        let options = c.read_u32::<BigEndian>()?;
        let datestamp = c.read_u32::<BigEndian>()?;
        let creation_day = c.read_u16::<BigEndian>()?;
        let creation_minute = c.read_u16::<BigEndian>()?;
        let creation_tick = c.read_u16::<BigEndian>()?;
        let protection = c.read_u16::<BigEndian>()?;

        let namelen = data[0x14] as usize;
        let namelen = namelen.min(31);
        let diskname = crate::util::latin1_to_string(&data[0x15..0x15 + namelen]);

        let mut c = Cursor::new(&data[0x34..]);
        let lastreserved = c.read_u32::<BigEndian>()?;
        let firstreserved = c.read_u32::<BigEndian>()?;
        let reserved_free = c.read_u32::<BigEndian>()?;
        let reserved_blksize = c.read_u16::<BigEndian>()?;
        let rblkcluster = c.read_u16::<BigEndian>()?;
        let blocksfree = c.read_u32::<BigEndian>()?;
        let alwaysfree = c.read_u32::<BigEndian>()?;
        let roving_ptr = c.read_u32::<BigEndian>()?;
        let deldir = c.read_u32::<BigEndian>()?;
        let disksize = c.read_u32::<BigEndian>()?;
        let extension = c.read_u32::<BigEndian>()?;

        let is_large = options & MODE_SUPERINDEX != 0;
        let mut bitmapindex = Vec::new();
        let mut indexblocks = Vec::new();

        let idx_base = 0x60;
        if is_large {
            for i in 0..=MAXBITMAPINDEX {
                let off = idx_base + i * 4;
                if off + 4 <= data.len() {
                    let v = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
                    bitmapindex.push(v);
                }
            }
        } else {
            for i in 0..=MAXSMALLBITMAPINDEX {
                let off = idx_base + i * 4;
                if off + 4 <= data.len() {
                    let v = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
                    bitmapindex.push(v);
                }
            }
            let idx_start = idx_base + (MAXSMALLBITMAPINDEX + 1) * 4;
            for i in 0..=MAXSMALLINDEXNR {
                let off = idx_start + i * 4;
                if off + 4 <= data.len() {
                    let v = u32::from_be_bytes(data[off..off + 4].try_into().unwrap());
                    indexblocks.push(v);
                }
            }
        }

        Ok(Self {
            disktype,
            options,
            datestamp,
            creation_day,
            creation_minute,
            creation_tick,
            protection,
            diskname,
            lastreserved,
            firstreserved,
            reserved_free,
            reserved_blksize,
            rblkcluster,
            blocksfree,
            alwaysfree,
            roving_ptr,
            deldir,
            disksize,
            extension,
            bitmapindex,
            indexblocks,
        })
    }

    pub fn has_flag(&self, flag: u32) -> bool {
        self.options & flag != 0
    }
    pub fn is_large(&self) -> bool {
        self.has_flag(MODE_SUPERINDEX)
    }
    pub fn has_extension(&self) -> bool {
        self.has_flag(MODE_EXTENSION) && self.extension != 0
    }
    pub fn has_longfn(&self) -> bool {
        self.has_flag(MODE_LONGFN)
    }
    pub fn has_largefile(&self) -> bool {
        self.has_flag(MODE_LARGEFILE)
    }
    pub fn is_splitted_anodes(&self) -> bool {
        self.has_flag(MODE_SPLITTED_ANODES)
    }

    pub fn flags_string(&self) -> String {
        let names = [
            (MODE_HARDDISK, "HARDDISK"),
            (MODE_SPLITTED_ANODES, "SPLITTED_ANODES"),
            (MODE_DIR_EXTENSION, "DIR_EXTENSION"),
            (MODE_DELDIR, "DELDIR"),
            (MODE_SIZEFIELD, "SIZEFIELD"),
            (MODE_EXTENSION, "EXTENSION"),
            (MODE_DATESTAMP, "DATESTAMP"),
            (MODE_SUPERINDEX, "SUPERINDEX"),
            (MODE_SUPERDELDIR, "SUPERDELDIR"),
            (MODE_EXTROVING, "EXTROVING"),
            (MODE_LONGFN, "LONGFN"),
            (MODE_LARGEFILE, "LARGEFILE"),
        ];
        names
            .iter()
            .filter(|(f, _)| self.options & f != 0)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// Rootblock extension — additional metadata for PFS3 v2+.
#[derive(Debug, Clone)]
pub struct RootblockExt {
    pub id: u16,
    pub ext_options: u32,
    pub datestamp: u32,
    pub pfs2version: u32,
    pub root_date: (u16, u16, u16),
    pub volume_date: (u16, u16, u16),
    pub reserved_roving: u32,
    pub fnsize: u16,
    pub superindex: Vec<u32>,
    pub deldirblocks: Vec<u32>,
}

impl RootblockExt {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x40 {
            return Err(Error::TooShort("rootblock extension"));
        }
        let id = u16::from_be_bytes(data[0..2].try_into().unwrap());
        if id != EXTENSIONID {
            return Err(Error::BadBlockId("rootblock extension", EXTENSIONID, id));
        }

        let mut c = Cursor::new(&data[0x04..]);
        let ext_options = c.read_u32::<BigEndian>()?;
        let datestamp = c.read_u32::<BigEndian>()?;
        let pfs2version = c.read_u32::<BigEndian>()?;

        let mut c = Cursor::new(&data[0x10..]);
        let root_date = (
            c.read_u16::<BigEndian>()?,
            c.read_u16::<BigEndian>()?,
            c.read_u16::<BigEndian>()?,
        );
        let volume_date = (
            c.read_u16::<BigEndian>()?,
            c.read_u16::<BigEndian>()?,
            c.read_u16::<BigEndian>()?,
        );

        let reserved_roving = u32::from_be_bytes(data[0x2C..0x30].try_into().unwrap());
        let mut fnsize = u16::from_be_bytes(data[0x38..0x3A].try_into().unwrap());
        if fnsize == 0 {
            fnsize = 32;
        }

        let mut superindex = Vec::new();
        for i in 0..=MAXSUPER {
            let off = 0x40 + i * 4;
            if off + 4 <= data.len() {
                superindex.push(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()));
            }
        }

        let mut deldirblocks = Vec::new();
        for i in 0..32 {
            let off = 0x90 + i * 4;
            if off + 4 <= data.len() {
                deldirblocks.push(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()));
            }
        }

        Ok(Self {
            id,
            ext_options,
            datestamp,
            pfs2version,
            root_date,
            volume_date,
            reserved_roving,
            fnsize,
            superindex,
            deldirblocks,
        })
    }
}
