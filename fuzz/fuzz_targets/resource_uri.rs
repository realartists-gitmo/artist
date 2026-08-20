#![no_main]

use artist_kernel::ResourceUri;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(uri) = std::str::from_utf8(bytes) {
        let _ = uri.parse::<ResourceUri>();
    }
});
