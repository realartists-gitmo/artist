#![no_main]

use artist_agent::protocol::RpcFrame;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(line) = std::str::from_utf8(bytes) {
        let _ = RpcFrame::from_line(line);
    }
});
