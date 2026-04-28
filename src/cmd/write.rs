use anyhow::Result;
use libpfs3::writer::Writer;

pub fn undelete(w: &mut Writer, name: &str, dest: Option<&str>) -> Result<()> {
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
