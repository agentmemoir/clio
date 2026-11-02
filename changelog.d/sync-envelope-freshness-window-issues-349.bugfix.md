Sync apply now rejects an envelope whose authenticated timestamp is far in the future. Such a row is recorded as a dead letter with reason `stale_envelope`, so an operator sees it in the pull acknowledgement instead of a silent duplicate that looks already applied.

Sync apply now keeps a durable per-device high-water mark, so a replayed envelope from a device is rejected by ordering even after the local mutation journal loses that id.

An environment whose sync backlog is legitimately old is unaffected, because the past-age bound ships disabled by default.
