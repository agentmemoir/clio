Sync apply now refuses a replayed envelope by a durable per-device, per-entity-kind ordering mark, so replay protection no longer depends on the mutation-id journal alone.

An envelope at or below the newest stamp already applied from the same device and kind is recorded as a dead letter with reason `stale_envelope`, so an operator sees it in the pull acknowledgement instead of a silent duplicate that looks already applied.

Both wall-clock bounds ship off: a legitimate backlog can be arbitrarily old, and some entity kinds stamp the envelope with a valid time or a transaction time rather than the change time, so a legitimate row can carry a stamp in the future. Each bound stays configurable for a deployment whose stamps are known to be change times.
