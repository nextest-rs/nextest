# Partitioning test runs in CI

For CI scenarios where test runs take too long on a single machine, nextest supports automatically *partitioning* or *sharding* tests into buckets.

cargo-nextest supports both *hash-based* and *count-based* partitioning. Here's an example of count-based partitioning, running tests in bucket 1 of 2.

<img src="https://user-images.githubusercontent.com/180618/153311562-f7a1b194-a968-4d08-b4c9-67afae19da72.png"/>

Tests not in the current bucket are marked skipped.

Count-based partitioning is done *per test binary*. This means that the tests in one binary *do not* influence counting for other binaries.

Hash-based partitioning is similar, except buckets are specified in the format `hash:m/n`, where `m` is the current bucket and `n` is the number of buckets. The hash is completely deterministic, and is based on a combination of the binary and test names. For sufficiently large test runs, hash-based partitioning produces roughly the same number of tests per bucket.
