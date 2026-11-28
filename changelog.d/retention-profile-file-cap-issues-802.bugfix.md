`clio retention set --profile-file` now refuses a profile file larger than the 4 MiB input cap with an error naming the cap, instead of reading the whole file into memory.
