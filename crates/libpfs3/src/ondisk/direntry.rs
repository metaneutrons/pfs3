//! Directory block headers and directory entry parsing.

use super::*;
use crate::error::{Error, Result};

/// Directory block header (0x14 bytes).
#[derive(Debug)]
pub struct DirBlockHeader {
    pub id: u16,
    pub datestamp: u32,
    pub anodenr: u32,
    pub parent: u32,
}

impl DirBlockHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x14 {
            return Err(Error::TooShort("dir block header"));
        }
        let id = u16::from_be_bytes(data[0..2].try_into().unwrap());
        if id != DBLKID {
            return Err(Error::BadBlockId("dir block", DBLKID, id));
        }
        let datestamp = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let anodenr = u32::from_be_bytes(data[0x0C..0x10].try_into().unwrap());
        let parent = u32::from_be_bytes(data[0x10..0x14].try_into().unwrap());
        Ok(Self {
            id,
            datestamp,
            anodenr,
            parent,
        })
    }
}

pub const DIR_BLOCK_HEADER_SIZE: usize = 0x14;

/// Variable-length directory entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub entry_size: u8,
    pub entry_type: i8,
    pub anode: u32,
    pub fsize: u32,
    pub creation_day: u16,
    pub creation_minute: u16,
    pub creation_tick: u16,
    pub protection: u8,
    pub name: String,
    pub comment: String,
    pub extra: ExtraFields,
}

/// Extra fields appended after name+comment in a directory entry.
#[derive(Debug, Clone, Default)]
pub struct ExtraFields {
    pub link: u32,
    pub uid: u16,
    pub gid: u16,
    pub prot: u32,
    pub virtualsize: u32,
    pub rollpointer: u32,
    pub fsizex: u16,
}

impl DirEntry {
    /// Parse one direntry from `data` at `offset`.
    /// Returns `(entry, next_offset)` or `None` if end/invalid.
    pub fn parse(data: &[u8], offset: usize) -> Option<(Self, usize)> {
        if offset >= data.len() {
            return None;
        }
        let entry_size = data[offset];
        if entry_size == 0 {
            return None;
        }
        let end = offset + entry_size as usize;
        if end > data.len() || (entry_size as usize) < 18 {
            return None;
        }
        let raw = &data[offset..end];

        let entry_type = raw[1] as i8;
        let anode = u32::from_be_bytes(raw[2..6].try_into().unwrap());
        let fsize = u32::from_be_bytes(raw[6..10].try_into().unwrap());
        let creation_day = u16::from_be_bytes(raw[10..12].try_into().unwrap());
        let creation_minute = u16::from_be_bytes(raw[12..14].try_into().unwrap());
        let creation_tick = u16::from_be_bytes(raw[14..16].try_into().unwrap());
        let protection = raw[16];
        let nlength = raw[17] as usize;

        let name_end = (18 + nlength).min(raw.len());
        let name = crate::util::latin1_to_string(&raw[18..name_end]);

        let mut comment = String::new();
        let comment_off = 18 + nlength;
        if comment_off < raw.len() {
            let clen = raw[comment_off] as usize;
            let cstart = comment_off + 1;
            if cstart + clen <= raw.len() {
                comment = crate::util::latin1_to_string(&raw[cstart..cstart + clen]);
            }
        }

        let extra = Self::parse_extrafields(raw, nlength);

        Some((
            Self {
                entry_size,
                entry_type,
                anode,
                fsize,
                creation_day,
                creation_minute,
                creation_tick,
                protection,
                name,
                comment,
                extra,
            },
            end,
        ))
    }

    fn parse_extrafields(raw: &[u8], nlength: usize) -> ExtraFields {
        let mut ef = ExtraFields::default();
        let name_end = 18 + nlength;
        if name_end >= raw.len() {
            return ef;
        }
        let clen = raw[name_end] as usize;
        let mut field_start = name_end + 1 + clen;
        if field_start & 1 != 0 {
            field_start += 1;
        }
        if field_start + 2 > raw.len() {
            return ef;
        }

        let flags = u16::from_be_bytes(raw[field_start..field_start + 2].try_into().unwrap());
        let mut pos = field_start + 2;

        if flags & 0x0001 != 0 && pos + 4 <= raw.len() {
            ef.link = u32::from_be_bytes(raw[pos..pos + 4].try_into().unwrap());
            pos += 4;
        }
        if flags & 0x0002 != 0 && pos + 2 <= raw.len() {
            ef.uid = u16::from_be_bytes(raw[pos..pos + 2].try_into().unwrap());
            pos += 2;
        }
        if flags & 0x0004 != 0 && pos + 2 <= raw.len() {
            ef.gid = u16::from_be_bytes(raw[pos..pos + 2].try_into().unwrap());
            pos += 2;
        }
        if flags & 0x0008 != 0 && pos + 4 <= raw.len() {
            ef.prot = u32::from_be_bytes(raw[pos..pos + 4].try_into().unwrap());
            pos += 4;
        }
        if flags & 0x0010 != 0 && pos + 4 <= raw.len() {
            ef.virtualsize = u32::from_be_bytes(raw[pos..pos + 4].try_into().unwrap());
            pos += 4;
        }
        if flags & 0x0020 != 0 && pos + 4 <= raw.len() {
            ef.rollpointer = u32::from_be_bytes(raw[pos..pos + 4].try_into().unwrap());
            pos += 4;
        }
        if flags & 0x0040 != 0 && pos + 2 <= raw.len() {
            ef.fsizex = u16::from_be_bytes(raw[pos..pos + 2].try_into().unwrap());
        }
        ef
    }

    pub fn is_file(&self) -> bool {
        self.entry_type < 0 && self.entry_type != ST_ROLLOVERFILE
    }
    pub fn is_rollover(&self) -> bool {
        self.entry_type == ST_ROLLOVERFILE
    }
    pub fn is_dir(&self) -> bool {
        self.entry_type == ST_USERDIR
    }
    pub fn is_softlink(&self) -> bool {
        self.entry_type == ST_SOFTLINK
    }
    pub fn is_hardlink(&self) -> bool {
        self.entry_type == ST_LINKDIR || self.entry_type == ST_LINKFILE
    }

    /// Full file size including extended bits 32-47.
    pub fn file_size(&self) -> u64 {
        self.fsize as u64 | ((self.extra.fsizex as u64) << 32)
    }
}
