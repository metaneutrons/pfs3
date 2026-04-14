use anyhow::{Result, bail};
use libpfs3::io::{BlockDevice, FileBlockDevice};
use libpfs3::ondisk::{RB_OFF_DISKNAME, ROOTBLOCK};
use std::path::Path;

pub fn run(image: &Path, name: Option<&str>, offset: u64) -> Result<()> {
    if name.is_none() {
        bail!("Nothing to change. Use --name to set volume name.");
    }

    let dev = FileBlockDevice::open_rw(image, 512, offset, 0)?;
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
