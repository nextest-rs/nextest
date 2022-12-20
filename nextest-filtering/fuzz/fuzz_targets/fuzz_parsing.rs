#![no_main]

use libfuzzer_sys::fuzz_target;
use nextest_filtering::Expr;

fuzz_target!(|data: &[u8]| {
    let input = match std::str::from_utf8(data) {
        Ok(data) => data,
        Err(_) => return,
    };
    // Check that the input doesn't crash.
    _ = Expr::parse(&input);
});
