use anyhow::{Result, bail};
use libpfs3::util;
use libpfs3::writer::Writer;

pub fn run_writer(w: &mut Writer, path: &str, spec: &str) -> Result<()> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        bail!("cannot set protection on root directory");
    }
    let name = parts.last().unwrap();
    let entry = w
        .vol
        .lookup(path)?
        .ok_or_else(|| anyhow::anyhow!("not found: {}", path))?;

    let new_prot = util::parse_amiga_protection(entry.protection, spec)
        .ok_or_else(|| anyhow::anyhow!("invalid protection spec: {}", spec))?;

    let parent_anode = if parts.len() == 1 {
        libpfs3::ondisk::ANODE_ROOTDIR
    } else {
        let parent_path = parts[..parts.len() - 1].join("/");
        w.vol
            .lookup(&parent_path)?
            .map(|e| e.anode)
            .unwrap_or(libpfs3::ondisk::ANODE_ROOTDIR)
    };

    w.update_dir_entry_protection(parent_anode, name, new_prot)?;
    println!(
        "{}: {} -> {}",
        path,
        util::amiga_protection_string(entry.protection),
        util::amiga_protection_string(new_prot)
    );
    Ok(())
}
