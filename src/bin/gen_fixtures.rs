//! Generate test fixture images using our own format + writer.
//! Replaces gen_fixtures.py — no Python or amitools dependency.
//!
//! Usage: cargo run --bin gen-fixtures

use libpfs3::format::{self, FormatOptions};
use libpfs3::io::FileBlockDevice;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;
use std::path::Path;

fn main() {
    let fixture_dir = Path::new("crates/libpfs3/tests/fixtures");
    std::fs::create_dir_all(fixture_dir).unwrap();
    generate_small_hdf(&fixture_dir.join("small.hdf"));
    println!("Done.");
}

fn generate_small_hdf(path: &Path) {
    let cyls: u32 = 10;
    let heads: u32 = 2;
    let secs: u32 = 32;
    let total_blocks = cyls * heads * secs;
    let image_size = total_blocks as usize * 512;

    let mut data = vec![0u8; image_size];
    write_rdb(&mut data, cyls, heads, secs);

    let part_start = (2 * heads * secs) as usize * 512;
    let part_blocks = ((cyls - 2) * heads * secs) as u64;

    // Write full image first, then format the partition in-place
    std::fs::write(path, &data).unwrap();

    let dev = FileBlockDevice::open_rw(path, 512, part_start as u64, part_blocks).unwrap();
    let opts = FormatOptions {
        volume_name: "TestVol".into(),
        enable_deldir: false,
    };
    format::format_with_size(&dev, part_blocks, &opts).unwrap();
    drop(dev);

    let dev = FileBlockDevice::open_rw(path, 512, part_start as u64, 0).unwrap();
    let vol = Volume::from_device(Box::new(dev)).unwrap();
    let mut w = Writer::open(vol).unwrap();
    w.write_file("hello.txt", b"Hello from PFS3!\n").unwrap();
    w.write_file(
        "test.bin",
        &(0..=255u8).cycle().take(1024).collect::<Vec<_>>(),
    )
    .unwrap();
    w.create_dir("SubDir").unwrap();
    w.write_file("SubDir/nested.txt", b"Nested file content.\n")
        .unwrap();

    println!("Generated {} ({} bytes)", path.display(), image_size);
}

fn write_rdb(data: &mut [u8], cyls: u32, heads: u32, secs: u32) {
    let mut rdsk = vec![0u8; 512];
    put_be(&mut rdsk, 0, u32::from_be_bytes(*b"RDSK"));
    put_be(&mut rdsk, 4, 64);
    put_be(&mut rdsk, 0x0C, 7);
    put_be(&mut rdsk, 0x10, 512);
    put_be(&mut rdsk, 0x1C, 1);
    put_be(&mut rdsk, 0x40, cyls);
    put_be(&mut rdsk, 0x44, secs);
    put_be(&mut rdsk, 0x48, heads);
    fix_checksum(&mut rdsk);
    data[..512].copy_from_slice(&rdsk);

    let mut part = vec![0u8; 512];
    put_be(&mut part, 0, u32::from_be_bytes(*b"PART"));
    put_be(&mut part, 4, 64);
    put_be(&mut part, 0x0C, 7);
    put_be(&mut part, 0x10, 0xFFFFFFFF);
    let name = b"DH0";
    part[0x24] = name.len() as u8;
    part[0x25..0x25 + name.len()].copy_from_slice(name);
    let env = 0x80;
    put_be(&mut part, env, 16);
    put_be(&mut part, env + 0x04, 128);
    put_be(&mut part, env + 0x0C, heads);
    put_be(&mut part, env + 0x10, 1);
    put_be(&mut part, env + 0x14, secs);
    put_be(&mut part, env + 0x18, 2);
    put_be(&mut part, env + 0x24, 2);
    put_be(&mut part, env + 0x28, cyls - 1);
    put_be(&mut part, env + 0x30, 1);
    put_be(&mut part, env + 0x34, 0x7FFFFFFE);
    put_be(&mut part, env + 0x40, 0x50465303);
    fix_checksum(&mut part);
    data[512..1024].copy_from_slice(&part);
}

fn put_be(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_be_bytes());
}

fn fix_checksum(block: &mut [u8]) {
    put_be(block, 8, 0);
    let mut total: u32 = 0;
    for i in (0..block.len()).step_by(4) {
        total = total.wrapping_add(u32::from_be_bytes(block[i..i + 4].try_into().unwrap()));
    }
    put_be(block, 8, 0u32.wrapping_sub(total));
}
