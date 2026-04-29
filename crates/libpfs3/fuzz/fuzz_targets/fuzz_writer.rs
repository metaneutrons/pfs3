#![no_main]
use libfuzzer_sys::fuzz_target;
use libpfs3::format::{self, FormatOptions};
use libpfs3::io::BlockDevice;
use libpfs3::volume::Volume;
use libpfs3::writer::Writer;
use std::sync::Mutex;

/// In-memory writable block device for fuzzing writer operations.
struct MemDev {
    data: Mutex<Vec<u8>>,
    bs: u32,
}

impl MemDev {
    fn new(blocks: u64, bs: u32) -> Self {
        Self {
            data: Mutex::new(vec![0u8; blocks as usize * bs as usize]),
            bs,
        }
    }
}

impl BlockDevice for MemDev {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> libpfs3::error::Result<()> {
        let data = self.data.lock().unwrap();
        let off = block as usize * self.bs as usize;
        let end = off + self.bs as usize;
        if end <= data.len() {
            buf[..self.bs as usize].copy_from_slice(&data[off..end]);
        }
        Ok(())
    }

    fn read_blocks(&self, block: u64, count: u32, buf: &mut [u8]) -> libpfs3::error::Result<()> {
        for i in 0..count {
            self.read_block(block + i as u64, &mut buf[i as usize * self.bs as usize..])?;
        }
        Ok(())
    }

    fn block_size(&self) -> u32 {
        self.bs
    }

    fn write_block(&self, block: u64, write_data: &[u8]) -> libpfs3::error::Result<()> {
        let mut data = self.data.lock().unwrap();
        let off = block as usize * self.bs as usize;
        let end = off + self.bs as usize;
        if end <= data.len() {
            data[off..end].copy_from_slice(&write_data[..self.bs as usize]);
        }
        Ok(())
    }

    fn write_blocks(&self, block: u64, count: u32, data: &[u8]) -> libpfs3::error::Result<()> {
        for i in 0..count {
            self.write_block(block + i as u64, &data[i as usize * self.bs as usize..])?;
        }
        Ok(())
    }

    fn flush(&self) -> libpfs3::error::Result<()> {
        Ok(())
    }
}

/// Operations the fuzzer can perform.
#[derive(Debug)]
enum Op {
    WriteFile { name_idx: u8, size: u16 },
    CreateDir { name_idx: u8 },
    Delete { name_idx: u8 },
    Overwrite { name_idx: u8, size: u16 },
}

fn parse_ops(data: &[u8]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut i = 0;
    while i + 4 <= data.len() {
        let op = match data[i] % 4 {
            0 => Op::WriteFile {
                name_idx: data[i + 1] % 16,
                size: u16::from_le_bytes([data[i + 2], data[i + 3]]) % 4096,
            },
            1 => Op::CreateDir {
                name_idx: data[i + 1] % 8,
            },
            2 => Op::Delete {
                name_idx: data[i + 1] % 16,
            },
            _ => Op::Overwrite {
                name_idx: data[i + 1] % 16,
                size: u16::from_le_bytes([data[i + 2], data[i + 3]]) % 4096,
            },
        };
        ops.push(op);
        i += 4;
    }
    ops
}

fn name_for(idx: u8) -> String {
    format!("f{}", idx)
}

fn dir_name_for(idx: u8) -> String {
    format!("d{}", idx)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }

    let blocks: u64 = 4096;
    let dev = MemDev::new(blocks, 512);
    let opts = FormatOptions {
        volume_name: "Fuzz".into(),
        enable_deldir: false,
    };
    if format::format_with_size(&dev, blocks, &opts).is_err() {
        return;
    }

    let vol = match Volume::from_device(Box::new(MemDev::new(0, 512))) {
        Ok(_) => return, // won't work with empty dev
        Err(_) => {}
    };

    // Reopen from the formatted device
    let formatted = dev.data.lock().unwrap().clone();
    let dev2 = MemDev::new(blocks, 512);
    *dev2.data.lock().unwrap() = formatted;

    let vol = match Volume::from_device(Box::new(dev2)) {
        Ok(v) => v,
        Err(_) => return,
    };

    let mut w = match Writer::open(vol) {
        Ok(w) => w,
        Err(_) => return,
    };

    let ops = parse_ops(data);
    for op in &ops {
        match op {
            Op::WriteFile { name_idx, size } => {
                let content = vec![*name_idx; *size as usize];
                let _ = w.write_file(&name_for(*name_idx), &content);
            }
            Op::CreateDir { name_idx } => {
                let _ = w.create_dir(&dir_name_for(*name_idx));
            }
            Op::Delete { name_idx } => {
                let _ = w.delete(&name_for(*name_idx));
            }
            Op::Overwrite { name_idx, size } => {
                let content = vec![name_idx.wrapping_add(1); *size as usize];
                let _ = w.write_file(&name_for(*name_idx), &content);
            }
        }
    }

    // After all operations, verify the volume is consistent:
    // list_dir must not panic
    let _ = w.vol.list_dir("/");
});
