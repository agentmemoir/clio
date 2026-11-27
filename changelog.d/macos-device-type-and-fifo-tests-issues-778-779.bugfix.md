Clio now builds on macOS and its tests compile: device numbers compare without a `u64`/`i32` mismatch, and named-pipe tests call libc on macOS, where rustix does not provide `mknodat` or `mkfifoat`.
