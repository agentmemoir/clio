`clio admit --file` now refuses a file larger than the 4 MiB input cap with a usage error and exit 2, instead of reading the whole file into memory.
