use anyhow::{Result, bail};
use libpfs3::io::{BlockDevice, FileBlockDevice};
use libpfs3::ondisk::{RB_OFF_DISKNAME, ROOTBLOCK};
use libpfs3::volume::Volume;
use std::path::Path;

pub fn run(image: &Path, name: Option<&str>, offset: u64, partition: Option<&str>) -> Result<()> {
    if name.is_none() {
        bail!("Nothing to change. Use --name to set volume name.");
    }

    let actual_offset = if let Some(part) = partition {
        let parts = libpfs3::volume::detect_pfs3_partitions(image)?;
        let p = parts
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(part))
            .or_else(|| part.parse::<usize>().ok().and_then(|i| parts.get(i)))
            .ok_or_else(|| {
                let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
                anyhow::anyhow!(
                    "partition '{}' not found (available: {})",
                    part,
                    names.join(", ")
                )
            })?;
        p.offset
    } else if offset == 0 {
        let vol = Volume::open_auto(image, 0, None, false)?;
        // Use the device's partition_offset — but since we opened successfully,
        // just detect the same way open_auto does
        drop(vol);
        libpfs3::rdb::detect_pfs3_partition(image).unwrap_or(0)
    } else {
        offset
    };

    let dev = FileBlockDevice::open_rw(image, 512, actual_offset, 0)?;
    let mut rb = vec![0u8; 512];
    dev.read_block(ROOTBLOCK, &mut rb)?;

    if let Some(new_name) = name {
        let name_bytes = new_name.as_bytes();
        let len = name_bytes.len().min(30);
        for b in &mut rb[RB_OFF_DISKNAME..RB_OFF_DISKNAME + 32] {
            *b = 0;
        }
        rb[RB_OFF_DISKNAME] = len as u8;
        rb[RB_OFF_DISKNAME + 1..RB_OFF_DISKNAME + 1 + len].copy_from_slice(&name_bytes[..len]);
        println!("Volume name set to: {}", new_name);
    }

    dev.write_block(ROOTBLOCK, &rb)?;
    dev.flush()?;
    println!("Done.");
    Ok(())
}
