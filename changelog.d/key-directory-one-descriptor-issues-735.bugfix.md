Clio now opens the directory that holds its sync key once, without following a link and without asking the kernel for any permission on it, and judges, tightens, and re-reads the mode of that one held directory instead of looking the path up three times, so a local process with write access to the directory's parent can no longer swap the entry between the check and the `chmod` and have key material kept in a directory Clio never checked; a directory replaced while it was being checked is refused with the path and the reason.

A key directory the owner can see but cannot read — `0300`, `0100`, and `0000` — is opened and repaired as it was before this gate existed, where a read open would have reported `EACCES` and turned a mode Clio fixes into a refusal it could not fix.

A key directory owned by a user other than the one running Clio is now refused with the file gate's own owner wording and its mode left untouched, so the owner decision the issue asks for is made on the same held object as the type and the mode.

An `ENOTDIR` or `ELOOP` caused by an ancestor above the key directory is reported as the unreadable path it is, rather than as a claim about the shape of a final component no lookup ever reached.

Limit, unchanged by this fix: the gate ends at the `chmod`. The callers still create the key file by path afterwards, and the ancestors above the key directory are resolved by the kernel on the open and are not held, so a swap in either window is not closed here.
