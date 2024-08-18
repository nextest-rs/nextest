#!/bin/env bash
#:
#: name = "build-and-test-illumos"
#: variety = "basic"
#: target = "helios-latest"
#: rust_toolchain = "stable"

# Build and test on illumos.

exec .github/buildomat/build-and-test.sh illumos
