#![no_main]
use libfuzzer_sys::fuzz_target;
use libpfs3::ondisk::Rootblock;

fuzz_target!(|data: &[u8]| {
    let _ = Rootblock::parse(data);
});
