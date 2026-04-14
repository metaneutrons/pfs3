use anyhow::Result;
use libpfs3::volume::Volume;

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
        "Created:         {}/{}/{}",
        rb.creation_day, rb.creation_minute, rb.creation_tick
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
