use anyhow::{Result, bail};
use libpfs3::writer::Writer;

pub fn run_writer(w: &mut Writer, name: Option<&str>) -> Result<()> {
    let Some(new_name) = name else {
        bail!("Nothing to change. Use --name to set volume name.");
    };
    w.set_volume_name(new_name)?;
    println!("Volume name set to: {}", new_name);
    Ok(())
}
