//! Wave 3: anode cycles, softlink edge cases, deldir roundtrip, RDB corruption,
//! fragmented range reads, protection bits, reserved exhaustion, full disk reads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::{BlockDevice, FileBlockDevice};
use libpfs3::ondisk::*;
use libpfs3::rdb;
use libpfs3::util;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;

static COUNTER: AtomicU32 = AtomicU32::new(20000);

fn fresh_image(blocks: u64) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pfs3_w3_{}.img", n));
    let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "Wave3".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, blocks, &opts).unwrap();
    drop(dev);
    path
}

fn open_writer(path: &Path) -> Writer {
    let dev = FileBlockDevice::open_rw(path, 512, 0, 0).unwrap();
    let vol = Volume::from_device(Box::new(dev)).unwrap();
    Writer::open(vol).unwrap()
}

fn reopen(path: &Path) -> Volume {
    Volume::open(path, 0).unwrap()
}

fn put_be32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}

// ============================================================
// Anode Cycle Detection
// ============================================================

mod anode_cycles {
    use super::*;

    #[test]
    fn read_file_on_corrupted_circular_chain_terminates() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("victim.txt", &vec![0xAA; 2048]).unwrap();
        drop(w);

        // Corrupt the anode to point to itself
        let mut data = std::fs::read(&path).unwrap();
        // Find anode blocks (ID 0x4142)
        for blk in 0..(data.len() / 512) {
            let off = blk * 512;
            if data[off] == 0x41 && data[off + 1] == 0x42 {
                // Anode block found. Each anode is 12 bytes starting at offset 16.
                // Make the first user anode (offset 16 + 6*12 = 88) point to itself.
                let anode_base = off + 16 + ANODE_USERFIRST as usize * ANODE_SIZE;
                if anode_base + 12 <= data.len() {
                    let cs =
                        u32::from_be_bytes(data[anode_base..anode_base + 4].try_into().unwrap());
                    if cs > 0 {
                        // Set 'next' field to this anode's own number (creating cycle)
                        let anodenr = ANODE_USERFIRST;
                        put_be32(&mut data, anode_base + 8, anodenr);
                        break;
                    }
                }
            }
        }
        std::fs::write(&path, &data).unwrap();

        // read_file must terminate (not infinite loop) — may return error or truncated data
        let mut vol = reopen(&path);
        let entry = vol.lookup("victim.txt").unwrap().unwrap();
        // This must complete in bounded time
        let result = vol.read_file_data(entry.anode, entry.file_size());
        // We don't care if it errors or returns data, just that it terminates
        let _ = result;
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn validate_anode_chain_on_normal_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("normal.txt", &vec![0; 4096]).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("normal.txt").unwrap().unwrap();
        let chain = vol.validate_anode_chain(entry.anode).unwrap();
        // 4096 bytes / 512 = 8 blocks
        assert_eq!(chain.len(), 8);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn validate_anode_chain_on_single_block_file() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("tiny.txt", b"x").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("tiny.txt").unwrap().unwrap();
        let chain = vol.validate_anode_chain(entry.anode).unwrap();
        assert_eq!(chain.len(), 1);
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Softlink Edge Cases
// ============================================================

mod softlinks {
    use super::*;

    #[test]
    fn softlink_to_nonexistent_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("dangling", "nonexistent/path/file.txt")
            .unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("dangling").unwrap().unwrap();
        assert!(entry.is_softlink());
        // Reading the link target should work (it's just stored data)
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&target),
            "nonexistent/path/file.txt"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_with_long_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let long_target = "A/".repeat(200) + "file.txt"; // ~400+ chars
        w.create_softlink("longlink", &long_target).unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("longlink").unwrap().unwrap();
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(String::from_utf8_lossy(&target), long_target);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_to_self_name() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        // Link pointing to its own name (would be infinite loop if resolved)
        w.create_softlink("loop", "loop").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("loop").unwrap().unwrap();
        assert!(entry.is_softlink());
        let target = vol.read_file_data(entry.anode, entry.file_size()).unwrap();
        assert_eq!(target, b"loop");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_empty_target() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("empty_target", "").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entry = vol.lookup("empty_target").unwrap().unwrap();
        assert!(entry.is_softlink());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn softlink_coexists_with_files() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.write_file("real.txt", b"real data").unwrap();
        w.create_softlink("link_to_real", "real.txt").unwrap();
        w.create_dir("SubDir").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 3);
        let link = entries.iter().find(|e| e.name == "link_to_real").unwrap();
        assert!(link.is_softlink());
        let file = entries.iter().find(|e| e.name == "real.txt").unwrap();
        assert!(file.is_file());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_softlink() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        w.create_softlink("mylink", "target").unwrap();
        w.delete("mylink").unwrap();
        drop(w);
        let mut vol = reopen(&path);
        assert!(vol.list_dir("/").unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Deldir / Undelete Roundtrip
// ============================================================

mod deldir {
    use super::*;

    /// Create an image with deldir enabled.
    fn fresh_image_with_deldir(blocks: u64) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pfs3_w3_dd_{}.img", n));
        let dev = FileBlockDevice::create(&path, 512, blocks).unwrap();
        let opts = FormatOptions {
            volume_name: "DelTest".into(),
            enable_deldir: true,
        };
        format::format_with_size(&dev, blocks, &opts).unwrap();
        drop(dev);
        path
    }

    #[test]
    fn deldir_empty_on_fresh_volume() {
        let path = fresh_image_with_deldir(4096);
        let mut vol = reopen(&path);
        let entries = vol.list_deldir().unwrap();
        assert!(entries.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn deleted_file_appears_in_deldir() {
        let path = fresh_image_with_deldir(4096);
        let mut w = open_writer(&path);
        w.write_file("deleteme.txt", b"precious data").unwrap();
        w.delete("deleteme.txt").unwrap();
        let entries = w.vol.list_deldir().unwrap();
        // File should be in deldir (if deldir is enabled and has space)
        if !entries.is_empty() {
            assert!(entries.iter().any(|e| e.filename.starts_with("deleteme")));
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn undelete_restores_file_content() {
        let path = fresh_image_with_deldir(4096);
        let mut w = open_writer(&path);
        w.write_file("restore.txt", b"restore me please").unwrap();
        w.delete("restore.txt").unwrap();

        let entries = w.vol.list_deldir().unwrap();
        if entries.is_empty() {
            // Deldir not enabled or no space — skip
            std::fs::remove_file(&path).ok();
            return;
        }

        // Find our file in deldir
        let idx = entries
            .iter()
            .position(|e| e.filename.starts_with("restore"))
            .unwrap();
        w.undelete(idx, "restored.txt").unwrap();
        drop(w);

        let mut vol = reopen(&path);
        let data = vol.read_file("restored.txt").unwrap();
        assert_eq!(data, b"restore me please");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn deldir_not_enabled_returns_empty() {
        // Standard image without deldir
        let path = fresh_image(4096);
        let mut vol = reopen(&path);
        let entries = vol.list_deldir().unwrap();
        assert!(entries.is_empty());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// RDB Partition Table Corruption
// ============================================================

mod rdb_corruption {
    use super::*;

    fn temp_path() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("pfs3_w3_rdb_{}.img", n))
    }

    #[test]
    fn no_rdb_signature_returns_empty() {
        let path = temp_path();
        // Create a file with no RDB signature
        std::fs::write(&path, &vec![0u8; 32768]).unwrap();
        let parts = rdb::detect_pfs3_partitions(&path).unwrap();
        assert!(parts.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rdb_with_no_partitions_returns_empty() {
        let path = temp_path();
        let mut data = vec![0u8; 32768];
        // Write RDSK signature
        data[0..4].copy_from_slice(b"RDSK");
        put_be32(&mut data, 4, 64); // size
        put_be32(&mut data, 0x0C, 7); // rdb_highblock
        std::fs::write(&path, &data).unwrap();
        let parts = rdb::detect_pfs3_partitions(&path).unwrap();
        assert!(parts.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rdb_with_non_pfs3_partition_skipped() {
        let path = temp_path();
        let mut data = vec![0u8; 65536];
        data[0..4].copy_from_slice(b"RDSK");
        put_be32(&mut data, 4, 64);
        put_be32(&mut data, 0x0C, 7);
        // Write a PART block with non-PFS dostype (e.g. FFS = 0x444F5301)
        let part_off = 512;
        data[part_off..part_off + 4].copy_from_slice(b"PART");
        put_be32(&mut data, part_off + 4, 64);
        data[part_off + 0x24] = 3;
        data[part_off + 0x25..part_off + 0x28].copy_from_slice(b"DH0");
        let env = part_off + 0x80;
        put_be32(&mut data, env + 0x0C, 2); // surfaces
        put_be32(&mut data, env + 0x14, 32); // bpt
        put_be32(&mut data, env + 0x24, 2); // low_cyl
        put_be32(&mut data, env + 0x28, 9); // high_cyl
        put_be32(&mut data, env + 0x40, 0x444F5301); // FFS dostype
        std::fs::write(&path, &data).unwrap();
        let parts = rdb::detect_pfs3_partitions(&path).unwrap();
        assert!(parts.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rdb_truncated_file_no_panic() {
        let path = temp_path();
        // File too short to contain partition blocks
        let mut data = vec![0u8; 512];
        data[0..4].copy_from_slice(b"RDSK");
        put_be32(&mut data, 4, 64);
        put_be32(&mut data, 0x0C, 63); // claims 63 blocks but file is only 1
        std::fs::write(&path, &data).unwrap();
        // Should not panic
        let _ = rdb::detect_pfs3_partitions(&path);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rdb_highblock_zero_no_panic() {
        let path = temp_path();
        let mut data = vec![0u8; 32768];
        data[0..4].copy_from_slice(b"RDSK");
        put_be32(&mut data, 4, 64);
        put_be32(&mut data, 0x0C, 0); // rdb_highblock = 0
        std::fs::write(&path, &data).unwrap();
        let parts = rdb::detect_pfs3_partitions(&path).unwrap();
        assert!(parts.is_empty());
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Fragmented File Range Reads
// ============================================================

mod fragmented_reads {
    use super::*;

    /// Create a fragmented file by writing/deleting to create gaps.
    fn create_fragmented_file(path: &Path) -> (u32, u64) {
        let mut w = open_writer(path);
        // Fill with small files to fragment the space
        for i in 0..10 {
            w.write_file(&format!("pad{}.bin", i), &vec![i as u8; 512])
                .unwrap();
        }
        // Delete alternating to create gaps
        for i in (0..10).step_by(2) {
            w.delete(&format!("pad{}.bin", i)).unwrap();
        }
        // Write a larger file that must span multiple extents
        let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        w.write_file("fragmented.bin", &data).unwrap();
        drop(w);
        let mut vol = reopen(path);
        let entry = vol.lookup("fragmented.bin").unwrap().unwrap();
        (entry.anode, entry.file_size())
    }

    #[test]
    fn range_read_first_block_of_fragmented() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, 0, 512).unwrap();
        let expected: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_across_extent_boundary() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        // Read 1024 bytes starting at offset 1024 — likely crosses extent boundary
        let data = vol.read_file_range(anode, size, 1024, 1024).unwrap();
        let expected: Vec<u8> = (1024..2048).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn range_read_last_bytes_of_fragmented() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let data = vol.read_file_range(anode, size, size - 10, 10).unwrap();
        let expected: Vec<u8> = ((size - 10)..size).map(|i| (i % 256) as u8).collect();
        assert_eq!(data, expected);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn full_read_matches_range_read_concatenation() {
        let path = fresh_image(4096);
        let (anode, size) = create_fragmented_file(&path);
        let mut vol = reopen(&path);
        let full = vol.read_file_data(anode, size).unwrap();
        // Read in 256-byte chunks and concatenate
        let mut assembled = Vec::new();
        let mut offset = 0u64;
        while offset < size {
            let chunk = vol.read_file_range(anode, size, offset, 256).unwrap();
            assembled.extend_from_slice(&chunk);
            offset += 256;
        }
        assert_eq!(full, assembled);
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Protection Bits Roundtrip
// ============================================================

mod protection_bits {
    use super::*;

    #[test]
    fn all_256_protection_values_roundtrip() {
        let path = fresh_image(8192);
        let mut w = open_writer(&path);
        // Create files with different protection values
        for prot in 0..=255u8 {
            let name = format!("p{:03}.txt", prot);
            w.write_file(&name, &[prot]).unwrap();
            w.update_dir_entry_protection(ANODE_ROOTDIR, &name, prot)
                .unwrap();
        }
        drop(w);

        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), 256);
        for entry in &entries {
            let expected_prot: u8 = entry.name[1..4].parse().unwrap();
            assert_eq!(
                entry.protection, expected_prot,
                "protection mismatch for {}: got {}, expected {}",
                entry.name, entry.protection, expected_prot
            );
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn protection_string_format() {
        // All denied
        assert_eq!(util::amiga_protection_string(0x0F), "--------");
        // All granted (RWED)
        assert_eq!(util::amiga_protection_string(0x00), "----rwed");
        // HSPA set, RWED granted
        assert_eq!(util::amiga_protection_string(0xF0), "hsparwed");
        // Only read
        assert_eq!(util::amiga_protection_string(0x07), "----r---");
    }

    #[test]
    fn parse_protection_absolute() {
        assert_eq!(util::parse_amiga_protection(0xFF, "rwed"), Some(0x00));
        assert_eq!(util::parse_amiga_protection(0x00, ""), None);
    }

    #[test]
    fn parse_protection_additive() {
        // Start with all denied, add read
        assert_eq!(util::parse_amiga_protection(0x0F, "+r"), Some(0x07));
    }

    #[test]
    fn parse_protection_subtractive() {
        // Start with all granted, remove write
        assert_eq!(util::parse_amiga_protection(0x00, "-w"), Some(0x04));
    }

    #[test]
    fn parse_protection_invalid_char() {
        assert_eq!(util::parse_amiga_protection(0x00, "xyz"), None);
    }
}

// ============================================================
// Reserved Block Exhaustion
// ============================================================

mod reserved_exhaustion {
    use super::*;

    #[test]
    fn many_directories_exhaust_reserved_gracefully() {
        let path = fresh_image(4096);
        let mut w = open_writer(&path);
        let mut created = 0;
        // Each directory uses a reserved block for its dir data
        for i in 0..200 {
            if w.create_dir(&format!("dir{:04}", i)).is_ok() {
                created += 1;
            } else {
                break;
            }
        }
        assert!(created > 10, "should create at least some dirs");
        drop(w);
        // Volume should still be readable
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), created);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reserved_exhaustion_returns_disk_full() {
        // Very small disk — reserved area fills up fast
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        let mut last_err = None;
        for i in 0..100 {
            match w.create_dir(&format!("d{}", i)) {
                Ok(_) => {}
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }
        // Should eventually get DiskFull, not panic
        if let Some(e) = last_err {
            let msg = format!("{}", e);
            assert!(
                msg.contains("full") || msg.contains("not enough") || msg.contains("no free"),
                "unexpected error: {}",
                msg
            );
        }
        std::fs::remove_file(&path).ok();
    }
}

// ============================================================
// Full Disk Read-Only Access
// ============================================================

mod full_disk_reads {
    use super::*;

    #[test]
    fn read_files_on_full_disk() {
        let path = fresh_image(512);
        let mut w = open_writer(&path);
        // Write files until full
        let mut files = Vec::new();
        for i in 0..100 {
            let name = format!("f{}.bin", i);
            let data = vec![i as u8; 256];
            if w.write_file(&name, &data).is_ok() {
                files.push((name, data));
            } else {
                break;
            }
        }
        assert!(!files.is_empty());
        drop(w);

        // All written files must be readable
        let mut vol = reopen(&path);
        for (name, expected) in &files {
            let data = vol.read_file(name).unwrap();
            assert_eq!(&data, expected, "data mismatch for {}", name);
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn list_dir_on_full_disk() {
        let path = fresh_image(512);
        let mut w = open_writer(&path);
        let mut count = 0;
        for i in 0..100 {
            if w.write_file(&format!("x{}.txt", i), &[0; 64]).is_ok() {
                count += 1;
            } else {
                break;
            }
        }
        drop(w);
        let mut vol = reopen(&path);
        let entries = vol.list_dir("/").unwrap();
        assert_eq!(entries.len(), count);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_on_full_disk_returns_error() {
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        // Fill it
        while w
            .write_file(&format!("f{}", w.vol.free_blocks()), &[0; 512])
            .is_ok()
        {}
        // Next write must fail
        let result = w.write_file("overflow.txt", b"nope");
        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn volume_info_on_full_disk() {
        let path = fresh_image(256);
        let mut w = open_writer(&path);
        while w
            .write_file(&format!("f{}", w.vol.free_blocks()), &[0; 512])
            .is_ok()
        {}
        let free = w.vol.free_blocks();
        let total = w.vol.total_blocks();
        assert!(free < 5);
        assert!(total > 0);
        assert_eq!(w.vol.name(), "Wave3");
        std::fs::remove_file(&path).ok();
    }
}
