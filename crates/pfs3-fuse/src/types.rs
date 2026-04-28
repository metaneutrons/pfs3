//! FUSE filesystem types, helpers, and inode management.

use std::collections::HashMap;
use std::sync::Mutex;

use fuser::{FileAttr, FileType};

use libpfs3::ondisk::*;
use libpfs3::util;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

pub const FUSE_ROOT_INO: u64 = 1;
pub const TRASHCAN_INO: u64 = 2;
/// Offset added to PFS3 anode numbers to produce FUSE inode numbers.
/// Must be greater than all virtual inodes (FUSE_ROOT_INO, TRASHCAN_INO).
const INODE_ANODE_OFFSET: u64 = 100;

pub struct InodeInfo {
    pub attr: FileAttr,
    pub anode: u32,
    pub parent_ino: u64,
    pub amiga_protection: u8,
    pub name: String,
}

/// Single-owner volume access. Either read-only Volume or Writer (which owns a Volume).
pub enum VolumeAccess {
    ReadOnly(Volume),
    ReadWrite(Writer),
}

impl VolumeAccess {
    pub fn vol(&self) -> &Volume {
        match self {
            VolumeAccess::ReadOnly(v) => v,
            VolumeAccess::ReadWrite(w) => &w.vol,
        }
    }

    pub fn vol_mut(&mut self) -> &mut Volume {
        match self {
            VolumeAccess::ReadOnly(v) => v,
            VolumeAccess::ReadWrite(w) => &mut w.vol,
        }
    }

    pub fn writer(&mut self) -> Option<&mut Writer> {
        match self {
            VolumeAccess::ReadWrite(w) => Some(w),
            _ => None,
        }
    }
}

pub struct DeldirEntry {
    pub ino: u64,
    pub anode: u32,
    pub size: u64,
    pub display_name: String,
    pub time: std::time::SystemTime,
}

pub struct FsInner {
    pub access: VolumeAccess,
    pub inodes: HashMap<u64, InodeInfo>,
    pub name_cache: HashMap<(u64, String), u64>,
    pub write_bufs: HashMap<u64, (u32, String, Vec<u8>)>,
    pub deldir: Vec<DeldirEntry>,
}

pub fn has_deldir(inner: &FsInner) -> bool {
    inner.access.vol().rootblock.has_flag(MODE_DELDIR)
}

pub fn rebuild_deldir(inner: &mut FsInner) {
    let entries = inner.access.vol_mut().list_deldir().unwrap_or_default();
    for old in &inner.deldir {
        inner.inodes.remove(&old.ino);
    }
    inner.deldir.clear();

    let mut name_counts: HashMap<String, u32> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let base = e.filename.clone();
        let count = name_counts.entry(base.to_ascii_lowercase()).or_insert(0);
        let display_name = if *count == 0 {
            base
        } else {
            format!("{}.{}", base, count)
        };
        *count += 1;

        let ino = TRASHCAN_INO + 1 + i as u64;
        let time = util::amiga_to_systime(e.creation_day, e.creation_minute, e.creation_tick);
        inner.deldir.push(DeldirEntry {
            ino,
            anode: e.anode,
            size: e.file_size(),
            display_name,
            time,
        });
    }
}

pub fn trashcan_attr(uid: u32, gid: u32) -> FileAttr {
    use std::time::SystemTime;
    FileAttr {
        ino: TRASHCAN_INO,
        size: 0,
        blocks: 0,
        atime: SystemTime::UNIX_EPOCH,
        mtime: SystemTime::UNIX_EPOCH,
        ctime: SystemTime::UNIX_EPOCH,
        crtime: SystemTime::UNIX_EPOCH,
        kind: FileType::Directory,
        perm: 0o555,
        nlink: 2,
        uid,
        gid,
        rdev: 0,
        blksize: 512,
        flags: 0,
    }
}

pub fn anode_to_ino(anode: u32) -> u64 {
    if anode == ANODE_ROOTDIR {
        FUSE_ROOT_INO
    } else {
        anode as u64 + INODE_ANODE_OFFSET
    }
}

pub fn make_attr(
    entry: &DirEntry,
    parent_ino: u64,
    uid: u32,
    gid: u32,
    bs: u32,
) -> (u64, InodeInfo) {
    let ino = anode_to_ino(entry.anode);
    let kind = if entry.is_dir() {
        FileType::Directory
    } else if entry.is_softlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    };
    let time = util::amiga_to_systime(
        entry.creation_day,
        entry.creation_minute,
        entry.creation_tick,
    );
    let size = entry.file_size();
    (
        ino,
        InodeInfo {
            attr: FileAttr {
                ino,
                size,
                blocks: size.div_ceil(bs as u64),
                atime: time,
                mtime: time,
                ctime: time,
                crtime: time,
                kind,
                perm: (util::amiga_protection_to_mode(entry.protection, entry.is_dir()) & 0o777)
                    as u16,
                nlink: if entry.is_dir() { 2 } else { 1 },
                uid,
                gid,
                rdev: 0,
                flags: 0,
                blksize: bs,
            },
            anode: entry.anode,
            parent_ino,
            amiga_protection: entry.protection,
            name: entry.name.clone(),
        },
    )
}

pub struct Pfs3Fs {
    pub inner: Mutex<FsInner>,
    pub uid: u32,
    pub gid: u32,
    pub block_size: u32,
    pub writable: bool,
}

impl Pfs3Fs {
    pub fn new(access: VolumeAccess) -> Self {
        let vol = access.vol();
        let block_size = vol.block_size();
        let rb = &vol.rootblock;
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let time = util::amiga_to_systime(rb.creation_day, rb.creation_minute, rb.creation_tick);

        let root_attr = FileAttr {
            ino: FUSE_ROOT_INO,
            size: 0,
            blocks: 0,
            atime: time,
            mtime: time,
            ctime: time,
            crtime: time,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid,
            gid,
            rdev: 0,
            flags: 0,
            blksize: block_size,
        };

        let mut inodes = HashMap::new();
        inodes.insert(
            FUSE_ROOT_INO,
            InodeInfo {
                attr: root_attr,
                anode: ANODE_ROOTDIR,
                parent_ino: FUSE_ROOT_INO,
                amiga_protection: 0,
                name: String::new(),
            },
        );

        let writable = matches!(&access, VolumeAccess::ReadWrite(_));
        let mut inner = FsInner {
            access,
            inodes,
            name_cache: HashMap::new(),
            write_bufs: HashMap::new(),
            deldir: Vec::new(),
        };
        rebuild_deldir(&mut inner);

        Self {
            inner: Mutex::new(inner),
            uid,
            gid,
            block_size,
            writable,
        }
    }
}
