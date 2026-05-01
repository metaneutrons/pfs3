use anyhow::Result;
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;
use std::path::Path;

pub fn run_vol(vol: &mut Volume, path: &str, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)?;

    // Resolve the path to a directory entry (or root)
    match vol.lookup(path)? {
        Some(entry) if entry.is_dir() => {
            extract_dir(vol, entry.anode, output, path)?;
        }
        Some(entry) => {
            // Single file
            let data = vol.read_file_data(entry.anode, entry.file_size())?;
            let name = path.rsplit('/').next().unwrap_or(path);
            let out_path = output.join(name);
            std::fs::write(&out_path, &data)?;
            println!("  {}", out_path.display());
        }
        None => {
            // Root directory
            extract_dir(vol, ANODE_ROOTDIR, output, "/")?;
        }
    }
    Ok(())
}

fn extract_dir(vol: &mut Volume, dir_anode: u32, output: &Path, display_path: &str) -> Result<()> {
    const MAX_DEPTH: usize = libpfs3::ondisk::MAX_DIR_DEPTH;
    if display_path.matches('/').count() > MAX_DEPTH {
        anyhow::bail!("directory nesting too deep at {}", display_path);
    }
    let entries = vol.list_dir_by_anode(dir_anode)?;
    for entry in &entries {
        let entry_path = libpfs3::util::join_pfs3_path(display_path, &entry.name);
        let out_path = output.join(&entry.name);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            extract_dir(vol, entry.anode, &out_path, &entry_path)?;
        } else if entry.is_file() {
            let data = vol.read_file_data(entry.anode, entry.file_size())?;
            std::fs::write(&out_path, &data)?;
            println!("  {}", out_path.display());
        }
    }
    Ok(())
}
