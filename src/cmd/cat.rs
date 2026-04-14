use anyhow::Result;
use libpfs3::volume::Volume;
use std::io::Write;

pub fn run_vol(vol: &mut Volume, path: &str) -> Result<()> {
    let data = vol.read_file(path)?;
    std::io::stdout().write_all(&data)?;
    Ok(())
}
