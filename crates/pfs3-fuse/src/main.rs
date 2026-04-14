//! pfs3-fuse — FUSE driver for PFS3 (Amiga) disk images.

mod types;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty,
    ReplyEntry, ReplyOpen, ReplyStatfs, Request,
};
use libc::{EIO, EISDIR, ENOENT, ENOTDIR, ENOTEMPTY, EPERM};

// ENODATA: Linux uses 61, macOS doesn't define it but ENOATTR (93) serves the same purpose.
#[cfg(target_os = "linux")]
const ENODATA: i32 = libc::ENODATA;
#[cfg(not(target_os = "linux"))]
const ENODATA: i32 = 93; // ENOATTR on macOS/BSD

// ENOTSUP: not in libc crate on all platforms
const ENOTSUP: i32 = 95;

use libpfs3::ondisk::*;
use libpfs3::util;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

use types::*;

const TTL: Duration = Duration::from_secs(3600);

#[derive(Parser)]
#[command(name = "pfs3-fuse", version, about = "Mount PFS3 disk images via FUSE")]
struct Args {
    image: PathBuf,
    mountpoint: PathBuf,
    #[arg(long, default_value = "0")]
    offset: u64,
    #[arg(long)]
    auto_unmount: bool,
    /// Enable EXPERIMENTAL read-write mode (risk of data corruption!)
    #[arg(long)]
    write: bool,
}

impl Filesystem for Pfs3Fs {
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        let inner = self.inner.lock().unwrap();
        if ino == TRASHCAN_INO && has_deldir(&inner) {
            reply.attr(&TTL, &trashcan_attr(self.uid, self.gid));
            return;
        }
        match inner.inodes.get(&ino) {
            Some(info) => reply.attr(&TTL, &info.attr),
            None => reply.error(ENOENT),
        }
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = match name.to_str() {
            Some(s) => s.to_string(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();

        // Virtual .Trashcan directory
        if parent == FUSE_ROOT_INO && name_str == ".Trashcan" && has_deldir(&inner) {
            let attr = trashcan_attr(self.uid, self.gid);
            reply.entry(&TTL, &attr, 0);
            return;
        }

        // Files inside .Trashcan — look up deldir snapshot
        if parent == TRASHCAN_INO && has_deldir(&inner) {
            if let Some(de) = inner
                .deldir
                .iter()
                .find(|de| de.display_name.eq_ignore_ascii_case(&name_str))
            {
                let de_ino = de.ino;
                let de_anode = de.anode;
                let de_size = de.size;
                let de_time = de.time;
                let de_display_name = de.display_name.clone();
                let attr = FileAttr {
                    ino: de_ino,
                    size: de_size,
                    blocks: (de_size + 511) / 512,
                    atime: de_time,
                    mtime: de_time,
                    ctime: de_time,
                    crtime: de_time,
                    kind: FileType::RegularFile,
                    perm: 0o444,
                    nlink: 1,
                    uid: self.uid,
                    gid: self.gid,
                    rdev: 0,
                    blksize: self.block_size,
                    flags: 0,
                };
                inner.inodes.insert(
                    de_ino,
                    InodeInfo {
                        attr,
                        anode: de_anode,
                        parent_ino: TRASHCAN_INO,
                        amiga_protection: 0,
                        name: de_display_name,
                    },
                );
                inner.name_cache.insert((parent, name_str), de_ino);
                reply.entry(&TTL, &attr, 0);
            } else {
                reply.error(ENOENT);
            }
            return;
        }

        if let Some(&ino) = inner.name_cache.get(&(parent, name_str.clone())) {
            if let Some(info) = inner.inodes.get(&ino) {
                reply.entry(&TTL, &info.attr, 0);
                return;
            }
        }

        let parent_anode = match inner.inodes.get(&parent) {
            Some(info) => info.anode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let entries = match inner.access.vol_mut().list_dir_by_anode(parent_anode) {
            Ok(e) => e,
            Err(_) => {
                reply.error(EIO);
                return;
            }
        };

        for entry in &entries {
            if util::name_eq_ci(&entry.name, &name_str) {
                let (ino, info) = make_attr(entry, parent, self.uid, self.gid, self.block_size);
                let attr = info.attr;
                inner.inodes.insert(ino, info);
                inner.name_cache.insert((parent, name_str), ino);
                reply.entry(&TTL, &attr, 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let mut inner = self.inner.lock().unwrap();

        // Virtual .Trashcan directory listing
        if ino == TRASHCAN_INO && has_deldir(&inner) {
            let mut full: Vec<(u64, FileType, String)> = vec![
                (TRASHCAN_INO, FileType::Directory, ".".into()),
                (FUSE_ROOT_INO, FileType::Directory, "..".into()),
            ];
            for de in &inner.deldir {
                full.push((de.ino, FileType::RegularFile, de.display_name.clone()));
            }
            for (i, (child_ino, kind, name)) in full.iter().enumerate().skip(offset as usize) {
                if reply.add(*child_ino, (i + 1) as i64, *kind, name) {
                    break;
                }
            }
            reply.ok();
            return;
        }

        let (anode, parent_ino) = match inner.inodes.get(&ino) {
            Some(info) if info.attr.kind == FileType::Directory => (info.anode, info.parent_ino),
            Some(_) => {
                reply.error(ENOTDIR);
                return;
            }
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let entries = match inner.access.vol_mut().list_dir_by_anode(anode) {
            Ok(e) => e,
            Err(_) => {
                reply.error(EIO);
                return;
            }
        };

        let mut full: Vec<(u64, FileType, String)> = vec![
            (ino, FileType::Directory, ".".into()),
            (parent_ino, FileType::Directory, "..".into()),
        ];
        for entry in &entries {
            let (child_ino, info) = make_attr(entry, ino, self.uid, self.gid, self.block_size);
            let kind = info.attr.kind;
            inner.inodes.insert(child_ino, info);
            full.push((child_ino, kind, entry.name.clone()));
        }

        // Add virtual .Trashcan to root listing
        if ino == FUSE_ROOT_INO && has_deldir(&inner) {
            full.push((TRASHCAN_INO, FileType::Directory, ".Trashcan".into()));
        }

        for (i, (child_ino, kind, name)) in full.iter().enumerate().skip(offset as usize) {
            if reply.add(*child_ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let mut inner = self.inner.lock().unwrap();
        match inner.inodes.get(&ino) {
            Some(info) if info.attr.kind == FileType::Directory => {
                reply.error(EISDIR);
                return;
            }
            None => {
                reply.error(ENOENT);
                return;
            }
            Some(_) => {}
        }

        // If opened for writing, load existing data into write buffer
        let write_flags = flags & (libc::O_WRONLY | libc::O_RDWR | libc::O_TRUNC | libc::O_APPEND);
        if self.writable && write_flags != 0 {
            let info = inner.inodes.get(&ino).unwrap();
            let anode = info.anode;
            let size = info.attr.size;
            let parent_ino = info.parent_ino;

            // Find the file's name by looking up the parent dir
            let parent_anode = inner
                .inodes
                .get(&parent_ino)
                .map(|i| i.anode)
                .unwrap_or(ANODE_ROOTDIR);
            let name = inner.inodes.get(&ino).map(|i| i.name.clone());

            if let Some(name) = name {
                let data = if flags & libc::O_TRUNC != 0 {
                    Vec::new()
                } else {
                    inner
                        .access
                        .vol_mut()
                        .read_file_data(anode, size)
                        .unwrap_or_default()
                };
                inner.write_bufs.insert(ino, (parent_anode, name, data));
            }
        }
        reply.opened(0, 0);
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let (anode, file_size) = match inner.inodes.get(&ino) {
            Some(info) => (info.anode, info.attr.size),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match inner
            .access
            .vol_mut()
            .read_file_range(anode, file_size, offset as u64, size)
        {
            Ok(data) => reply.data(&data),
            Err(_) => reply.error(EIO),
        }
    }

    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        let mut inner = self.inner.lock().unwrap();
        let (anode, size) = match inner.inodes.get(&ino) {
            Some(info) => (info.anode, info.attr.size),
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match inner.access.vol_mut().read_file_data(anode, size) {
            Ok(data) => reply.data(&data),
            Err(_) => reply.error(EIO),
        }
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        let inner = self.inner.lock().unwrap();
        let vol = inner.access.vol();
        let rb = &vol.rootblock;
        reply.statfs(
            rb.disksize as u64,
            rb.blocksfree as u64,
            rb.blocksfree as u64,
            0,
            0,
            vol.block_size(),
            vol.fnsize() as u32,
            0,
        );
    }

    // ---- Write operations ----

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(EIO);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();
        let parent_anode = match inner.inodes.get(&parent) {
            Some(info) if info.attr.kind == FileType::Directory => info.anode,
            _ => {
                reply.error(ENOENT);
                return;
            }
        };

        let w = match inner.access.writer() {
            Some(w) => w,
            None => {
                reply.error(EPERM);
                return;
            }
        };
        if w.create_dir_in(parent_anode, name_str).is_err() {
            reply.error(EIO);
            return;
        }

        // Re-read directory to find the new entry
        let entries = match w.vol.list_dir_by_anode(parent_anode) {
            Ok(e) => e,
            Err(_) => {
                reply.error(EIO);
                return;
            }
        };

        if let Some(entry) = entries.iter().find(|e| util::name_eq_ci(&e.name, name_str)) {
            let (ino, info) = make_attr(entry, parent, self.uid, self.gid, self.block_size);
            let attr = info.attr;
            inner.inodes.insert(ino, info);
            inner.name_cache.insert((parent, name_str.to_string()), ino);
            reply.entry(&TTL, &attr, 0);
        } else {
            reply.error(EIO);
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(EIO);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();
        let parent_anode = match inner.inodes.get(&parent) {
            Some(info) => info.anode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match inner.access.writer() {
            Some(w) => match w.delete_in(parent_anode, name_str) {
                Ok(()) => {
                    if let Some(ino) = inner.name_cache.remove(&(parent, name_str.to_string())) {
                        inner.inodes.remove(&ino);
                    }
                    rebuild_deldir(&mut inner);
                    reply.ok();
                }
                Err(_) => reply.error(EIO),
            },
            None => reply.error(EPERM),
        }
    }

    fn rmdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(EIO);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();
        let parent_anode = match inner.inodes.get(&parent) {
            Some(info) => info.anode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match inner.access.writer() {
            Some(w) => match w.delete_in(parent_anode, name_str) {
                Ok(()) => {
                    if let Some(ino) = inner.name_cache.remove(&(parent, name_str.to_string())) {
                        inner.inodes.remove(&ino);
                    }
                    reply.ok();
                }
                Err(_) => reply.error(ENOTEMPTY),
            },
            None => reply.error(EPERM),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let (src_name, dst_name) = match (name.to_str(), newname.to_str()) {
            (Some(s), Some(d)) => (s, d),
            _ => {
                reply.error(EIO);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();
        let src_anode = match inner.inodes.get(&parent) {
            Some(info) => info.anode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let dst_anode = match inner.inodes.get(&newparent) {
            Some(info) => info.anode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        match inner.access.writer() {
            Some(w) => match w.rename_in(src_anode, src_name, dst_anode, dst_name) {
                Ok(()) => {
                    // Update inode name and parent
                    if let Some(&ino) = inner.name_cache.get(&(parent, src_name.to_string())) {
                        if let Some(info) = inner.inodes.get_mut(&ino) {
                            info.name = dst_name.to_string();
                            info.parent_ino = newparent;
                        }
                        inner
                            .name_cache
                            .insert((newparent, dst_name.to_string()), ino);
                    }
                    inner.name_cache.remove(&(parent, src_name.to_string()));
                    reply.ok();
                }
                Err(_) => reply.error(EIO),
            },
            None => reply.error(EPERM),
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: fuser::ReplyWrite,
    ) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some((_, _, buf)) = inner.write_bufs.get_mut(&ino) {
            let off = offset as usize;
            let end = off + data.len();
            if end > buf.len() {
                buf.resize(end, 0);
            }
            buf[off..end].copy_from_slice(data);
            let new_size = buf.len() as u64;
            if let Some(info) = inner.inodes.get_mut(&ino) {
                info.attr.size = new_size;
            }
            reply.written(data.len() as u32);
        } else {
            reply.error(EIO);
        }
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: fuser::ReplyCreate,
    ) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(EIO);
                return;
            }
        };
        let mut inner = self.inner.lock().unwrap();
        let parent_anode = match inner.inodes.get(&parent) {
            Some(info) if info.attr.kind == FileType::Directory => info.anode,
            _ => {
                reply.error(ENOENT);
                return;
            }
        };

        let w = match inner.access.writer() {
            Some(w) => w,
            None => {
                reply.error(EPERM);
                return;
            }
        };
        if w.write_file_in(parent_anode, name_str, &[]).is_err() {
            reply.error(EIO);
            return;
        }

        let entries = match w.vol.list_dir_by_anode(parent_anode) {
            Ok(e) => e,
            Err(_) => {
                reply.error(EIO);
                return;
            }
        };
        if let Some(entry) = entries.iter().find(|e| util::name_eq_ci(&e.name, name_str)) {
            let (ino, info) = make_attr(entry, parent, self.uid, self.gid, self.block_size);
            let attr = info.attr;
            inner.inodes.insert(ino, info);
            inner.name_cache.insert((parent, name_str.to_string()), ino);
            inner
                .write_bufs
                .insert(ino, (parent_anode, name_str.to_string(), Vec::new()));
            reply.created(&TTL, &attr, 0, 0, 0);
        } else {
            reply.error(EIO);
        }
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if let Some((parent_anode, name, data)) = inner.write_bufs.remove(&ino) {
            if !data.is_empty() {
                let file_anode = inner.inodes.get(&ino).map(|i| i.anode).unwrap_or(0);
                let result = match inner.access.writer() {
                    Some(w) if file_anode != 0 => {
                        w.overwrite_file_in(parent_anode, &name, file_anode, &data)
                    }
                    _ => {
                        reply.ok();
                        return;
                    }
                };
                if result.is_err() {
                    reply.error(EIO);
                    return;
                }
                if let Some(info) = inner.inodes.get_mut(&ino) {
                    info.attr.size = data.len() as u64;
                    info.attr.blocks = data.len().div_ceil(self.block_size as usize) as u64;
                }
            }
        }
        reply.ok();
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(new_size) = size {
            if let Some(info) = inner.inodes.get_mut(&ino) {
                info.attr.size = new_size;
            }
            if let Some((_, _, buf)) = inner.write_bufs.get_mut(&ino) {
                buf.resize(new_size as usize, 0);
            }
        }
        if let Some(new_mode) = mode {
            if !self.writable {
                reply.error(EPERM);
                return;
            }
            // Extract all needed values in one pass to avoid borrow issues
            let inode_data = inner.inodes.get(&ino).map(|i| {
                (
                    i.name.clone(),
                    i.parent_ino,
                    i.amiga_protection,
                    i.attr.kind == FileType::Directory,
                )
            });
            if let Some((name, parent_ino, old_prot, is_dir)) = inode_data {
                let parent_anode = inner.inodes.get(&parent_ino).map(|p| p.anode);
                if let Some(pa) = parent_anode {
                    let amiga_prot =
                        (old_prot & 0xF0) | (util::unix_mode_to_amiga_protection(new_mode) & 0x0F);
                    if let Some(w) = inner.access.writer() {
                        if w.update_dir_entry_protection(pa, &name, amiga_prot)
                            .is_err()
                        {
                            reply.error(EIO);
                            return;
                        }
                    }
                    if let Some(info) = inner.inodes.get_mut(&ino) {
                        info.attr.perm =
                            (util::amiga_protection_to_mode(amiga_prot, is_dir) & 0o777) as u16;
                        info.amiga_protection = amiga_prot;
                    }
                }
            }
        }
        if let Some(info) = inner.inodes.get(&ino) {
            reply.attr(&TTL, &info.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        size: u32,
        reply: fuser::ReplyXattr,
    ) {
        // Root and virtual .Trashcan have no dir entries, so no protection bits
        if ino == FUSE_ROOT_INO || ino == TRASHCAN_INO {
            reply.error(ENODATA);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        if name_str != "user.amiga.protection" {
            reply.error(ENODATA);
            return;
        }
        let inner = self.inner.lock().unwrap();
        let prot = match inner.inodes.get(&ino) {
            Some(info) => info.amiga_protection,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let val = util::amiga_protection_string(prot);
        if size == 0 {
            reply.size(val.len() as u32);
        } else if size < val.len() as u32 {
            reply.error(libc::ERANGE);
        } else {
            reply.data(val.as_bytes());
        }
    }

    fn setxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        if !self.writable {
            reply.error(EPERM);
            return;
        }
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(EIO);
                return;
            }
        };
        if name_str != "user.amiga.protection" {
            reply.error(ENOTSUP);
            return;
        }
        let spec = match std::str::from_utf8(value) {
            Ok(s) => s.trim(),
            Err(_) => {
                reply.error(EIO);
                return;
            }
        };
        // Parse the protection spec (same format as CLI: "rwed", "+p", "-wd")
        let mut inner = self.inner.lock().unwrap();
        let current_prot = inner
            .inodes
            .get(&ino)
            .map(|i| i.amiga_protection)
            .unwrap_or(0);
        let new_prot = match util::parse_amiga_protection(current_prot, spec) {
            Some(p) => p,
            None => {
                reply.error(EIO);
                return;
            }
        };
        // Find file name and parent for disk write
        let inode_data = inner.inodes.get(&ino).map(|i| {
            (
                i.name.clone(),
                i.parent_ino,
                i.attr.kind == FileType::Directory,
            )
        });
        let (file_name, parent_ino, is_dir) = match inode_data {
            Some(d) => d,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        let parent_anode = inner.inodes.get(&parent_ino).map(|p| p.anode);

        if let Some(pa) = parent_anode {
            if let Some(w) = inner.access.writer() {
                if w.update_dir_entry_protection(pa, &file_name, new_prot)
                    .is_err()
                {
                    reply.error(EIO);
                    return;
                }
            }
            if let Some(info) = inner.inodes.get_mut(&ino) {
                info.attr.perm = (util::amiga_protection_to_mode(new_prot, is_dir) & 0o777) as u16;
                info.amiga_protection = new_prot;
            }
        }
        reply.ok();
    }

    fn listxattr(&mut self, _req: &Request<'_>, ino: u64, size: u32, reply: fuser::ReplyXattr) {
        let inner = self.inner.lock().unwrap();
        if !inner.inodes.contains_key(&ino) {
            reply.error(ENOENT);
            return;
        }
        if ino == FUSE_ROOT_INO || ino == TRASHCAN_INO {
            if size == 0 {
                reply.size(0);
            } else {
                reply.data(&[]);
            }
            return;
        }
        // "user.amiga.protection\0"
        let list = b"user.amiga.protection\0";
        if size == 0 {
            reply.size(list.len() as u32);
        } else if size < list.len() as u32 {
            reply.error(libc::ERANGE);
        } else {
            reply.data(list);
        }
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();

    let rw = args.write;
    if rw {
        eprintln!("WARNING: Read-write mode is EXPERIMENTAL and may cause DATA CORRUPTION.");
        eprintln!("         The PFS3 write path does not implement atomic updates.");
        eprintln!("         Use only on copies of disk images, never on originals.");
        eprintln!();
    }

    let access = if rw {
        let vol = Volume::open_auto(&args.image, args.offset, None, true)?;
        VolumeAccess::ReadWrite(Writer::open(vol)?)
    } else {
        VolumeAccess::ReadOnly(Volume::open_auto(&args.image, args.offset, None, false)?)
    };

    let vol = access.vol();
    eprintln!(
        "Mounting PFS3 volume \"{}\" at {} ({})",
        vol.name(),
        args.mountpoint.display(),
        if rw {
            "EXPERIMENTAL read-write"
        } else {
            "read-only"
        }
    );
    eprintln!(
        "  {} blocks, {} free, block size {}",
        vol.total_blocks(),
        vol.free_blocks(),
        vol.block_size()
    );

    let fs = Pfs3Fs::new(access);

    let mut options = vec![
        MountOption::FSName("pfs3".to_string()),
        MountOption::DefaultPermissions,
    ];
    if !rw {
        options.push(MountOption::RO);
    }
    if args.auto_unmount {
        options.push(MountOption::AutoUnmount);
    }

    fuser::mount2(fs, &args.mountpoint, &options)?;
    Ok(())
}
