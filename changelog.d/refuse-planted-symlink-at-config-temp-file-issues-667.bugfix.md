The config writer now refuses an entry already planted at its temp file name. Before this change the temp path was opened with truncate, so a symlink planted there was followed and the file it pointed at was truncated.
The temp file is now created new, so an existing entry at that name fails that write with `AlreadyExists`, and the planted entry and its target stay unchanged.
The write leaves the temp file in place when creation is refused, so the failure does not delete an entry the writer did not create.
A real target that already exists is untouched: the backup writer keeps its existing write path and still replaces a file in place.
