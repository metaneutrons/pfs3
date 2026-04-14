//! RDB (Rigid Disk Block) partition detection for Amiga disk images.

use std::path::Path;

use crate::error::{Error, Result};
use crate::ondisk::PFS_TYPES;

/// RDB dostype identifiers for PFS3-compatible filesystem handlers.
/// These appear in the partition environment vector, not in the rootblock.
/// PFS\3 = pfs3aio handler, PDS\3 = pds3 (Professional DOS) handler.
const RDB_PFS3_DOSTYPES: &[u32] = &[
    0x5046_5301, // PFS\1
    0x5046_5302, // PFS\2
    0x5046_5303, // PFS\3
    0x5044_5301, // PDS\1
    0x5044_5302, // PDS\2
    0x5044_5303, // PDS\3
];

/// Info about a detected PFS3 partition in an RDB image.
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    /// Partition name (e.g. "DH0").
    pub name: String,
    /// Byte offset to partition start.
    pub offset: u64,
    /// Partition size in blocks.
    pub blocks: u64,
}

/// Detect all PFS3 partitions in an RDB disk image.
pub fn detect_pfs3_partitions(path: &Path) -> Result<Vec<PartitionInfo>> {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};

    let mut f = File::open(path)?;
    let mut buf = [0u8; 512];

    f.read_exact(&mut buf)?;
    let sig = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    if sig != 0x5244_534B {
        return Ok(Vec::new()); // not RDB
    }

    let mut partitions = Vec::new();
    for blk in 1..64 {
        f.seek(SeekFrom::Start(blk * 512))?;
        if f.read(&mut buf)? < 512 {
            break;
        }
        if u32::from_be_bytes(buf[0..4].try_into().unwrap()) != 0x5041_5254 {
            continue;
        }

        let nlen = buf[0x24] as usize;
        let name = crate::util::latin1_to_string(&buf[0x25..0x25 + nlen.min(30)]);

        let env_off = 0x80;
        if env_off + 0x44 > 512 {
            continue;
        }
        let surfaces = u32::from_be_bytes(buf[env_off + 0x0C..env_off + 0x10].try_into().unwrap());
        let bpt = u32::from_be_bytes(buf[env_off + 0x14..env_off + 0x18].try_into().unwrap());
        let low_cyl = u32::from_be_bytes(buf[env_off + 0x24..env_off + 0x28].try_into().unwrap());
        let high_cyl = u32::from_be_bytes(buf[env_off + 0x28..env_off + 0x2C].try_into().unwrap());
        let dostype = u32::from_be_bytes(buf[env_off + 0x40..env_off + 0x44].try_into().unwrap());

        if RDB_PFS3_DOSTYPES.contains(&dostype) || PFS_TYPES.contains(&dostype) {
            let offset = low_cyl as u64 * surfaces as u64 * bpt as u64 * 512;
            let blocks = (high_cyl - low_cyl + 1) as u64 * surfaces as u64 * bpt as u64;
            partitions.push(PartitionInfo {
                name,
                offset,
                blocks,
            });
        }
    }
    Ok(partitions)
}

/// Detect first PFS3 partition offset in an RDB disk image.
pub fn detect_pfs3_partition(path: &Path) -> Result<u64> {
    let parts = detect_pfs3_partitions(path)?;
    parts
        .first()
        .map(|p| p.offset)
        .ok_or_else(|| Error::InvalidPartition("no PFS3 partition found in RDB image".into()))
}
