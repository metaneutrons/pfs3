use anyhow::Result;
use libpfs3::util;
use libpfs3::volume::Volume;

pub fn run_vol(vol: &mut Volume, path: &str) -> Result<()> {
    let entries = vol.list_dir(path)?;
    println!(
        "{:<8} {:<10} {:>12} {:<20} Name",
        "Type", "Protect", "Size", "Date"
    );
    println!("{}", "-".repeat(72));
    for entry in &entries {
        let kind = if entry.is_dir() {
            "DIR"
        } else if entry.is_softlink() {
            "LINK"
        } else {
            "FILE"
        };
        let size = if entry.is_dir() {
            String::new()
        } else {
            format!("{}", entry.file_size())
        };
        let date = util::amiga_date_string(
            entry.creation_day,
            entry.creation_minute,
            entry.creation_tick,
        );
        let prot = util::amiga_protection_string(entry.protection);
        println!(
            "{:<8} {:<10} {:>12} {:<20} {}",
            kind, prot, size, date, entry.name
        );
    }
    println!("\n{} entries", entries.len());
    Ok(())
}
