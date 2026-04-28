use anyhow::Result;
use libpfs3::rdb::PartitionInfo;
use libpfs3::volume::Volume;
use std::path::Path;

/// Show detailed info for a single volume.
pub fn run_vol(vol: Volume) -> Result<()> {
    let rb = &vol.rootblock;
    println!("PFS3 Volume Information");
    println!("=======================");
    println!("Disk name:       {}", vol.name());
    println!("Total blocks:    {}", vol.total_blocks());
    println!("Free blocks:     {}", vol.free_blocks());
    println!("Block size:      {} bytes", vol.block_size());
    println!("Reserved blksize:{} bytes", rb.reserved_blksize);
    println!("First reserved:  {}", rb.firstreserved);
    println!("Last reserved:   {}", rb.lastreserved);
    println!("Reserved free:   {}", rb.reserved_free);
    println!("Options:         0x{:04X}", rb.options);
    println!("Datestamp:       {}", rb.datestamp);
    println!(
        "Created:         {}",
        libpfs3::util::amiga_date_string(rb.creation_day, rb.creation_minute, rb.creation_tick)
    );
    println!("Filename length: {}", vol.fnsize());

    println!("Flags:           {}", rb.flags_string());

    if let Some(ref ext) = vol.rootblock_ext {
        println!("\nRootblock Extension:");
        println!("  PFS2 version:  0x{:08X}", ext.pfs2version);
        println!("  Superindex:    {} entries", ext.superindex.len());
        println!("  Deldir blocks: {} entries", ext.deldirblocks.len());
    }

    Ok(())
}

/// Show overview of all partitions in an RDB image.
pub fn run_overview(image: &Path, partitions: &[PartitionInfo]) -> Result<()> {
    println!("PFS3 Disk Image: {}", image.display());
    println!("Partitions: {}\n", partitions.len());

    for part in partitions {
        let vol = Volume::open(image, part.offset)?;
        let rb = &vol.rootblock;
        let created =
            libpfs3::util::amiga_date_string(rb.creation_day, rb.creation_minute, rb.creation_tick);
        println!("[{}] {}", part.name, vol.name());
        println!(
            "  Blocks: {} total, {} free, {} bytes/block",
            vol.total_blocks(),
            vol.free_blocks(),
            vol.block_size()
        );
        println!("  Created: {}", created);
        println!("  Flags:   {}\n", rb.flags_string());
    }

    println!("Use --partition <name> for detailed info.");
    Ok(())
}
