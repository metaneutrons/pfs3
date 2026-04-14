use anyhow::Result;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;
use std::path::Path;

fn open_writer(image: &Path, offset: u64, partition: Option<&str>) -> Result<Writer> {
    let vol = Volume::open_auto(image, offset, partition, true)?;
    Ok(Writer::open(vol)?)
}

pub fn run(
    image: &Path,
    src: &Path,
    dest: &str,
    offset: u64,
    partition: Option<&str>,
) -> Result<()> {
    let data = std::fs::read(src)?;
    let mut w = open_writer(image, offset, partition)?;
    w.write_file(dest, &data)?;
    println!("Wrote {} ({} bytes) -> {}", src.display(), data.len(), dest);
    Ok(())
}

pub fn mkdir(image: &Path, path: &str, offset: u64, partition: Option<&str>) -> Result<()> {
    let mut w = open_writer(image, offset, partition)?;
    w.create_dir(path)?;
    println!("Created directory: {}", path);
    Ok(())
}

pub fn rm(image: &Path, path: &str, offset: u64, partition: Option<&str>) -> Result<()> {
    let mut w = open_writer(image, offset, partition)?;
    w.delete(path)?;
    println!("Removed: {}", path);
    Ok(())
}

pub fn undelete(
    image: &Path,
    name: &str,
    dest: Option<&str>,
    offset: u64,
    partition: Option<&str>,
) -> Result<()> {
    let mut w = open_writer(image, offset, partition)?;

    // Find the deldir entry by name or index
    let entries = w.vol.list_deldir()?;
    if entries.is_empty() {
        anyhow::bail!("deldir is empty or not enabled");
    }

    let idx = if let Ok(i) = name.parse::<usize>() {
        if i >= entries.len() {
            anyhow::bail!("index {} out of range (0..{})", i, entries.len() - 1);
        }
        i
    } else {
        entries
            .iter()
            .position(|e| e.filename.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow::anyhow!("'{}' not found in deldir", name))?
    };

    let entry = &entries[idx];
    let dest_path = dest.unwrap_or(&entry.filename);
    println!(
        "Undeleting '{}' ({} bytes) -> {}",
        entry.filename,
        entry.file_size(),
        dest_path
    );
    w.undelete(idx, dest_path)?;
    println!("Done.");
    Ok(())
}
