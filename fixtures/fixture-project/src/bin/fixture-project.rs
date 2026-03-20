// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

fn main() {
    println!("The answer is 42");
    std::process::exit(42);
}

#[cfg(test)]
mod tests {
    #[test]
    fn bin_success() {}
}
