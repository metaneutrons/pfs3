use anyhow::{Result, bail};
use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use std::path::Path;

pub fn run(
    image: &Path,
    name: &str,
    size_mb: Option<u32>,
    offset: u64,
    partition: Option<&str>,
) -> Result<()> {
    let block_bytes: u32 = 512;

    let actual_offset = if let Some(part) = partition {
        let parts = libpfs3::volume::detect_pfs3_partitions(image)?;
        let p = parts
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(part))
            .or_else(|| part.parse::<usize>().ok().and_then(|i| parts.get(i)))
            .ok_or_else(|| {
                let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
                anyhow::anyhow!(
                    "partition '{}' not found (available: {})",
                    part,
                    names.join(", ")
                )
            })?;
        p.offset
    } else if offset == 0 {
        libpfs3::rdb::detect_pfs3_partition(image).unwrap_or(0)
    } else {
        offset
    };

    let (dev, total_blocks) = if let Some(mb) = size_mb {
        let total = mb as u64 * 1024 * 1024 / block_bytes as u64;
        let d = FileBlockDevice::create(image, block_bytes, total)?;
        (d, total)
    } else if image.exists() {
        let d = FileBlockDevice::open_rw(image, block_bytes, actual_offset, 0)?;
        let total = d.total_blocks();
        (d, total)
    } else {
        bail!("Image does not exist. Use --size-mb to create a new image.");
    };

    if total_blocks < 64 {
        bail!("Image too small (need at least 64 blocks)");
    }

    println!("Formatting {} as PFS3...", image.display());
    println!("  Volume name: {}", name);
    println!(
        "  Total blocks: {} ({} bytes)",
        total_blocks,
        total_blocks * block_bytes as u64
    );

    let opts = FormatOptions {
        volume_name: name.to_string(),
        enable_deldir: false,
    };

    let result = format::format_with_size(&dev, total_blocks, &opts)?;

    println!(
        "  Reserved blocks: {} (block size: {} bytes)",
        result.num_reserved, result.reserved_blksize
    );
    println!(
        "  Data blocks: {} ({} bytes free)",
        result.data_blocks,
        result.blocks_free * block_bytes as u64
    );
    println!("Done.");

    Ok(())
}
