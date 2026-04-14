#![deny(warnings)]
//! libpfs3 — PFS3 (Professional File System III) library.
//!
//! Pure Rust implementation of the Amiga PFS3 filesystem. Supports reading,
//! writing, formatting, and checking PFS3 disk images and raw partitions.
//!
//! # Reading files
//! ```no_run
//! use libpfs3::volume::Volume;
//! let mut vol = Volume::open_rdb(std::path::Path::new("disk.hdf")).unwrap();
//! for entry in vol.list_dir("/").unwrap() {
//!     println!("{}", entry.name);
//! }
//! let data = vol.read_file("S/Startup-Sequence").unwrap();
//! ```
//!
//! # Writing files
//! ```no_run
//! use libpfs3::volume::Volume;
//! use libpfs3::writer::Writer;
//! let vol = Volume::open_rdb(std::path::Path::new("disk.hdf")).unwrap();
//! let mut w = Writer::open(vol).unwrap();
//! w.write_file("hello.txt", b"Hello from Rust!").unwrap();
//! w.create_dir("NewDir").unwrap();
//! ```

pub mod anode;
pub mod bitmap;
pub mod cache;
pub mod dir;
pub mod error;
pub mod format;
pub mod io;
pub mod ondisk;
pub mod rdb;
pub mod util;
pub mod volume;
pub mod writer;
