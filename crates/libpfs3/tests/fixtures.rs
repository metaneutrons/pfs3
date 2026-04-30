// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025 Fabian Schmieder

//! Golden-vector tests against real PFS3 disk images.

use std::path::{Path, PathBuf};
use std::sync::Once;

use libpfs3::ondisk::*;
use libpfs3::volume::Volume;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn open_small() -> Volume {
    Volume::open_rdb(&fixtures_dir().join("small.hdf")).unwrap()
}

static EXTRACT_PFS: Once = Once::new();

fn open_pfs() -> Volume {
    let hdf = fixtures_dir().join("pfs.hdf");
    EXTRACT_PFS.call_once(|| {
        if !hdf.exists() {
            let archive = fixtures_dir().join("pfs.7z");
            assert!(archive.exists(), "pfs.7z fixture missing");
            sevenz_rust::decompress_file(&archive, &fixtures_dir())
                .expect("failed to extract pfs.7z");
        }
    });
    Volume::open_rdb(&hdf).unwrap()
}

// ============================================================
// Macro: generate a test module for each image
// ============================================================

macro_rules! tests_for_image {
    ($mod_name:ident, $opener:ident) => {
        mod $mod_name {
            use super::*;

            // -- Volume / rootblock --

            #[test]
            fn open_and_validate() {
                let vol = $opener();
                assert_eq!(vol.rootblock.disktype, ID_PFS_DISK);
                assert_eq!(vol.block_size(), 512);
                assert_eq!(vol.rootblock.reserved_blksize, 1024);
                assert!(vol.rootblock.is_splitted_anodes());
                assert!(vol.rootblock.has_extension());
                assert!(vol.rootblock_ext.is_some());
                assert!(!vol.name().is_empty());
                assert!(vol.total_blocks() > 0);
                assert!(vol.free_blocks() > 0);
                assert!(vol.free_blocks() < vol.total_blocks());
                assert!(vol.rootblock.firstreserved < vol.rootblock.lastreserved);
            }

            #[test]
            fn flags_string_nonempty() {
                let vol = $opener();
                let flags = vol.rootblock.flags_string();
                assert!(flags.contains("HARDDISK"));
                assert!(flags.contains("SPLITTED_ANODES"));
            }

            // -- Directory listing --

            #[test]
            fn list_root_nonempty() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                assert!(!entries.is_empty());
                // Every entry has a non-empty name
                for e in &entries {
                    assert!(!e.name.is_empty());
                }
            }

            #[test]
            fn root_has_files_and_dirs() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                assert!(entries.iter().any(|e| e.is_file()));
                assert!(entries.iter().any(|e| e.is_dir()));
            }

            #[test]
            fn list_first_subdir() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                // Find a dir that contains at least one file
                for dir in entries.iter().filter(|e| e.is_dir()) {
                    let sub = vol.list_dir(&dir.name).unwrap();
                    if sub.iter().any(|e| e.is_file()) {
                        return; // found one, pass
                    }
                }
                panic!("no subdirectory with files found");
            }

            // -- Entry types --

            #[test]
            fn file_entry_properties() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                assert!(file.entry_type < 0);
                assert!(file.file_size() > 0);
                assert!(file.creation_day > 0);
            }

            #[test]
            fn dir_entry_properties() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let dir = entries.iter().find(|e| e.is_dir()).unwrap();
                assert_eq!(dir.entry_type, ST_USERDIR);
            }

            // -- Path resolution --

            #[test]
            fn lookup_root_is_none() {
                let mut vol = $opener();
                assert!(vol.lookup("/").unwrap().is_none());
            }

            #[test]
            fn lookup_first_file() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                let found = vol.lookup(&file.name).unwrap().unwrap();
                assert_eq!(found.name, file.name);
                assert_eq!(found.file_size(), file.file_size());
            }

            #[test]
            fn lookup_case_insensitive() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                let upper = file.name.to_ascii_uppercase();
                let found = vol.lookup(&upper).unwrap().unwrap();
                assert_eq!(found.name, file.name);
            }

            #[test]
            fn lookup_nonexistent() {
                let mut vol = $opener();
                assert!(vol.lookup("__nonexistent_file_42__").unwrap().is_none());
            }

            #[test]
            fn lookup_nested_file() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                for dir in entries.iter().filter(|e| e.is_dir()) {
                    let sub = vol.list_dir(&dir.name).unwrap();
                    if let Some(file) = sub.iter().find(|e| e.is_file()) {
                        let path = format!("{}/{}", dir.name, file.name);
                        let found = vol.lookup(&path).unwrap().unwrap();
                        assert_eq!(found.name, file.name);
                        return;
                    }
                }
                panic!("no nested file found");
            }

            // -- File reading --

            #[test]
            fn read_first_file_size_matches() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                let data = vol.read_file(&file.name).unwrap();
                assert_eq!(data.len() as u64, file.file_size());
            }

            #[test]
            fn read_nested_file() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                for dir in entries.iter().filter(|e| e.is_dir()) {
                    let sub = vol.list_dir(&dir.name).unwrap();
                    if let Some(file) = sub.iter().find(|e| e.is_file()) {
                        let path = format!("{}/{}", dir.name, file.name);
                        let data = vol.read_file(&path).unwrap();
                        assert_eq!(data.len() as u64, file.file_size());
                        return;
                    }
                }
                panic!("no nested file found");
            }

            // -- Anode chains --

            #[test]
            fn anode_chain_rootdir() {
                let mut vol = $opener();
                let blocks = vol.validate_anode_chain(ANODE_ROOTDIR).unwrap();
                assert!(!blocks.is_empty());
            }

            #[test]
            fn anode_chain_matches_file_size() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                let blocks = vol.validate_anode_chain(file.anode).unwrap();
                let expected = (file.file_size() + 511) / 512;
                assert_eq!(blocks.len() as u64, expected);
            }

            // -- Error cases --

            #[test]
            fn read_dir_as_file_fails() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let dir = entries.iter().find(|e| e.is_dir()).unwrap();
                assert!(vol.read_file(&dir.name).is_err());
            }

            #[test]
            fn list_file_as_dir_fails() {
                let mut vol = $opener();
                let entries = vol.list_dir("/").unwrap();
                let file = entries.iter().find(|e| e.is_file()).unwrap();
                assert!(vol.list_dir(&file.name).is_err());
            }
        }
    };
}

tests_for_image!(small, open_small);
tests_for_image!(pfs_real, open_pfs);

// ============================================================
// Image-specific tests (known exact values)
// ============================================================

mod small_exact {
    use super::*;

    #[test]
    fn volume_name() {
        assert_eq!(open_small().name(), "TestVol");
    }

    #[test]
    fn total_blocks() {
        assert_eq!(open_small().total_blocks(), 512);
    }

    #[test]
    fn root_entry_count() {
        assert_eq!(open_small().list_dir("/").unwrap().len(), 3);
    }

    #[test]
    fn hello_content() {
        assert_eq!(
            open_small().read_file("hello.txt").unwrap(),
            b"Hello from PFS3!\n"
        );
    }

    #[test]
    fn binary_content() {
        let data = open_small().read_file("test.bin").unwrap();
        let expected: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
        assert_eq!(data, expected);
    }

    #[test]
    fn nested_content() {
        assert_eq!(
            open_small().read_file("SubDir/nested.txt").unwrap(),
            b"Nested file content.\n"
        );
    }
}

mod pfs_exact {
    use super::*;

    #[test]
    fn volume_name() {
        assert_eq!(open_pfs().name(), "PFS3AIO Volume");
    }

    #[test]
    fn total_blocks() {
        assert_eq!(open_pfs().total_blocks(), 15876);
    }

    #[test]
    fn has_deldir() {
        assert!(open_pfs().rootblock.has_flag(MODE_DELDIR));
    }

    #[test]
    fn root_entry_count() {
        assert_eq!(open_pfs().list_dir("/").unwrap().len(), 13);
    }

    #[test]
    fn libs_count() {
        assert_eq!(open_pfs().list_dir("Libs").unwrap().len(), 8);
    }

    #[test]
    fn startup_sequence_content() {
        let data = open_pfs().read_file("S/Startup-Sequence").unwrap();
        assert!(String::from_utf8_lossy(&data).contains("xSysInfo"));
    }

    #[test]
    fn library_size() {
        let e = open_pfs().lookup("Libs/68060.library").unwrap().unwrap();
        assert_eq!(e.file_size(), 65476);
    }

    #[test]
    fn sysinfo_guide_size() {
        let e = open_pfs().lookup("SysInfo/SysInfo.guide").unwrap().unwrap();
        assert_eq!(e.file_size(), 39019);
    }
}
