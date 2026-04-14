#![no_main]
use libfuzzer_sys::fuzz_target;
use libpfs3::io::BlockDevice;
use libpfs3::volume::Volume;

/// In-memory block device for fuzzing.
struct MemDev {
    data: Vec<u8>,
    bs: u32,
}

impl BlockDevice for MemDev {
    fn read_block(&self, block: u64, buf: &mut [u8]) -> libpfs3::error::Result<()> {
        let off = (block as usize).checked_mul(self.bs as usize).unwrap_or(usize::MAX);
        let end = off.checked_add(self.bs as usize).unwrap_or(usize::MAX);
        if end <= self.data.len() {
            buf[..self.bs as usize].copy_from_slice(&self.data[off..end]);
        }
        Ok(())
    }
    fn read_blocks(&self, block: u64, count: u32, buf: &mut [u8]) -> libpfs3::error::Result<()> {
        for i in 0..count {
            let buf_off = i as usize * self.bs as usize;
            if buf_off + self.bs as usize <= buf.len() {
                self.read_block(block + i as u64, &mut buf[buf_off..])?;
            }
        }
        Ok(())
    }
    fn block_size(&self) -> u32 { self.bs }
    fn write_block(&self, _: u64, _: &[u8]) -> libpfs3::error::Result<()> { Ok(()) }
    fn write_blocks(&self, _: u64, _: u32, _: &[u8]) -> libpfs3::error::Result<()> { Ok(()) }
    fn flush(&self) -> libpfs3::error::Result<()> { Ok(()) }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2048 { return; }
    let dev = Box::new(MemDev { data: data.to_vec(), bs: 512 });
    let _ = Volume::from_device(dev);
});
