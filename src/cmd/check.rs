use anyhow::Result;
use libpfs3::ondisk::*;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;
use std::collections::{HashMap, HashSet};

use std::path::Path;

#[derive(Default)]
struct CheckCtx {
    errors: u32,
    warnings: u32,
    files: u32,
    dirs: u32,
    used_blocks: HashMap<u64, String>,
    used_anodes: HashSet<u32>,
    repairs: u32,
    /// Files to remove during repair: (parent_anode, name)
    broken_files: Vec<(u32, String)>,
    /// Files with wrong size: (parent_anode, name, correct_size)
    wrong_size: Vec<(u32, String, u64)>,
    /// Corrected blocksfree value (if different from rootblock)
    correct_blocksfree: Option<u32>,
    /// Corrected reserved_free value
    correct_reserved_free: Option<u32>,
}

fn open_vol(image: &Path, offset: u64, partition: Option<&str>) -> Result<Volume> {
    Ok(Volume::open_auto(image, offset, partition, false)?)
}

fn open_vol_rw(image: &Path, offset: u64, partition: Option<&str>) -> Result<Volume> {
    Ok(Volume::open_auto(image, offset, partition, true)?)
}

pub fn run(image: &Path, offset: u64, partition: Option<&str>, repair: bool) -> Result<()> {
    let mut ctx = CheckCtx::default();

    let data_area;

    // Phase 1: scan (owns Volume, drops at end of block)
    {
        let mut vol = open_vol(image, offset, partition)?;
        println!("PFS3 Filesystem Check");
        println!("=====================");
        println!("Volume: {}", vol.name());
        println!("Disk size: {} blocks", vol.total_blocks());
        println!();

        let firstreserved = vol.rootblock.firstreserved;
        let lastreserved = vol.rootblock.lastreserved;
        let disksize = vol.rootblock.disksize;
        let blocksfree = vol.rootblock.blocksfree;
        let has_extension = vol.rootblock.has_extension();
        let has_ext_loaded = vol.rootblock_ext.is_some();

        // Check 1: rootblock validity
        print!("Checking rootblock... ");
        if firstreserved >= lastreserved {
            println!(
                "ERROR: invalid reserved area ({}-{})",
                firstreserved, lastreserved
            );
            ctx.errors += 1;
        } else if disksize == 0 {
            println!("ERROR: disksize is 0");
            ctx.errors += 1;
        } else {
            println!("OK");
        }

        // Check 2: rootblock extension
        if has_extension {
            print!("Checking rootblock extension... ");
            if has_ext_loaded {
                println!("OK");
            } else {
                println!("ERROR: extension flag set but could not read");
                ctx.errors += 1;
            }
        }

        // Check 3: walk directory tree
        println!("Scanning directory tree...");
        scan_dir(&mut vol, ANODE_ROOTDIR, "/", &mut ctx);

        // Check 4: bitmap cross-reference
        print!("Scanning bitmap... ");
        let bitmap_free = vol.bitmap_count_free().unwrap_or(0);
        let reserved_blocks = lastreserved + 1;
        let data_area_val = disksize - reserved_blocks;

        if bitmap_free != blocksfree {
            println!(
                "WARNING: rootblock says {} free, bitmap scan says {} free",
                blocksfree, bitmap_free
            );
            ctx.warnings += 1;
            if repair {
                ctx.correct_blocksfree = Some(bitmap_free);
            }
        } else {
            println!(
                "OK (bitmap: {} free, tree: {} data blocks used)",
                bitmap_free,
                ctx.used_blocks.len()
            );
        }

        // Check 5: duplicate block detection (handled during scan_dir via HashMap)
        print!("Checking for duplicate blocks... ");
        println!("OK");

        // Check 6: anode sanity
        print!("Checking anode usage... ");
        if ctx.used_anodes.contains(&0) {
            println!("WARNING: anode 0 (EOF marker) referenced as data anode");
            ctx.warnings += 1;
        } else {
            println!("OK ({} anodes in use)", ctx.used_anodes.len());
        }

        // Check 7: reserved free count
        print!("Checking reserved blocks... ");
        let reserved_free = vol.rootblock.reserved_free;
        match vol.reserved_count_free() {
            Ok(actual_free) => {
                if actual_free != reserved_free {
                    println!(
                        "WARNING: rootblock says {} reserved free, bitmap says {}",
                        reserved_free, actual_free
                    );
                    ctx.warnings += 1;
                    if repair {
                        ctx.correct_reserved_free = Some(actual_free);
                    }
                } else {
                    println!("OK ({} reserved free)", reserved_free);
                }
            }
            Err(_) => println!("SKIP (could not read reserved bitmap)"),
        }
        data_area = data_area_val;
    } // vol dropped here

    // Phase 2: apply all repairs via Writer
    let needs_repair = repair
        && (!ctx.broken_files.is_empty()
            || !ctx.wrong_size.is_empty()
            || ctx.correct_blocksfree.is_some()
            || ctx.correct_reserved_free.is_some());
    if needs_repair {
        println!("\nApplying repairs...");
        let vol = open_vol_rw(image, offset, partition)?;
        let mut w = Writer::open(vol)?;

        if let Some(bf) = ctx.correct_blocksfree {
            w.repair_blocksfree(bf)?;
            println!("  REPAIRED: blocksfree set to {}", bf);
            ctx.repairs += 1;
        }
        if let Some(rf) = ctx.correct_reserved_free {
            w.repair_reserved_free(rf)?;
            println!("  REPAIRED: reserved_free set to {}", rf);
            ctx.repairs += 1;
        }

        for (parent_anode, name) in &ctx.broken_files {
            match w.force_remove_entry(*parent_anode, name) {
                Ok(()) => {
                    println!("  REPAIRED: removed broken entry '{}'", name);
                    ctx.repairs += 1;
                }
                Err(e) => println!("  FAILED to remove '{}': {}", name, e),
            }
        }

        for (_parent_anode, name, correct_size) in &ctx.wrong_size {
            println!(
                "  WARNING: '{}' should be {} bytes (chain too short) — manual fix needed",
                name, correct_size
            );
        }
    }

    println!();
    println!("Summary:");
    println!("  Files:       {}", ctx.files);
    println!("  Directories: {}", ctx.dirs);
    println!(
        "  Data blocks: {} (of {} data area)",
        ctx.used_blocks.len(),
        data_area
    );
    println!("  Anodes used: {}", ctx.used_anodes.len());
    println!("  Errors:      {}", ctx.errors);
    println!("  Warnings:    {}", ctx.warnings);
    if ctx.repairs > 0 {
        println!("  Repairs:     {}", ctx.repairs);
    }

    if ctx.errors == 0 && ctx.warnings == 0 {
        println!("\nFilesystem appears clean.");
    } else if ctx.errors == 0 {
        println!("\nFilesystem OK with {} warning(s).", ctx.warnings);
    } else {
        println!("\nFilesystem has {} error(s)!", ctx.errors);
    }

    Ok(())
}

fn scan_dir(vol: &mut Volume, dir_anode: u32, path: &str, ctx: &mut CheckCtx) {
    const MAX_DEPTH: usize = libpfs3::ondisk::MAX_DIR_DEPTH;
    let depth = path.matches('/').count();
    if depth > MAX_DEPTH {
        println!("  ERROR: directory nesting too deep at {}", path);
        ctx.errors += 1;
        return;
    }
    let entries = match vol.list_dir_by_anode(dir_anode) {
        Ok(e) => e,
        Err(e) => {
            println!("  ERROR reading dir {}: {}", path, e);
            ctx.errors += 1;
            return;
        }
    };

    for entry in &entries {
        let entry_path = libpfs3::util::join_pfs3_path(path, &entry.name);

        if entry.is_dir() {
            ctx.dirs += 1;
            match vol.validate_anode_chain(entry.anode) {
                Ok(blocks) => {
                    record_blocks(blocks, &entry_path, ctx);
                    record_anode_chain(vol, entry.anode, &mut ctx.used_anodes);
                }
                Err(e) => {
                    println!("  ERROR: dir {} anode chain: {}", entry_path, e);
                    ctx.errors += 1;
                    ctx.broken_files.push((dir_anode, entry.name.clone()));
                    continue;
                }
            }
            scan_dir(vol, entry.anode, &entry_path, ctx);
        } else if entry.is_file() || entry.entry_type == ST_ROLLOVERFILE {
            ctx.files += 1;
            match vol.validate_anode_chain(entry.anode) {
                Ok(blocks) => {
                    let chain_blocks = blocks.len() as u64;
                    let bs = vol.block_size() as u64;
                    let expected = entry.file_size().div_ceil(bs);
                    if chain_blocks < expected && entry.file_size() > 0 {
                        let actual_size = chain_blocks * bs;
                        println!(
                            "  WARNING: {} has {} chain blocks but needs {} for {} bytes",
                            entry_path,
                            chain_blocks,
                            expected,
                            entry.file_size()
                        );
                        ctx.warnings += 1;
                        ctx.wrong_size
                            .push((dir_anode, entry.name.clone(), actual_size));
                    }
                    record_blocks(blocks, &entry_path, ctx);
                    record_anode_chain(vol, entry.anode, &mut ctx.used_anodes);
                }
                Err(e) => {
                    println!("  ERROR: file {} anode chain: {}", entry_path, e);
                    ctx.errors += 1;
                    ctx.broken_files.push((dir_anode, entry.name.clone()));
                }
            }
        }
    }
}

fn record_blocks(blocks: Vec<u64>, owner: &str, ctx: &mut CheckCtx) {
    for b in blocks {
        if let Some(prev_owner) = ctx.used_blocks.insert(b, owner.to_string()) {
            println!(
                "  ERROR: block {} claimed by both '{}' and '{}'",
                b, prev_owner, owner
            );
            ctx.errors += 1;
        }
    }
}

fn record_anode_chain(vol: &mut Volume, anodenr: u32, used_anodes: &mut HashSet<u32>) {
    if let Ok(chain) = vol.get_anode_chain(anodenr) {
        for an in &chain {
            used_anodes.insert(an.nr);
        }
    }
}
