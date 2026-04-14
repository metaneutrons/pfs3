#![no_main]
use libfuzzer_sys::fuzz_target;
use libpfs3::ondisk::DirEntry;

fuzz_target!(|data: &[u8]| {
    let _ = DirEntry::parse(data, 0);
});
