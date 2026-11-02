Sync apply now keeps an ordering mark per device and per entity kind, and refuses an envelope whose stamp is strictly below the newest stamp already applied from that device for that kind. Such an envelope is refused without consulting the mutation-id journal, so a replay is caught even when the journal no longer holds the original mutation. This defence holds only while a device keeps the same identity: a reinstall or a reset that issues a new device id starts a new mark.

A stamp equal to the mark is accepted. Stamps have one-second granularity, so two real mutations from one device can carry the same stamp, and refusing an equal stamp would dead-letter real traffic. An exact repeat of that stamp is still caught by the mutation-id journal.

A refused envelope is recorded as a dead letter with reason `stale_envelope`, so an operator sees it in the pull acknowledgement instead of a silent duplicate that looks already applied.

Both wall-clock bounds ship disabled by default: the past-age bound at `0` seconds and the future-dated bound at `0` seconds. A legitimate backlog can be arbitrarily old, and some entity kinds stamp the envelope with a valid time or a transaction time rather than the change time, so a legitimate row can carry a stamp in the future. Each bound stays configurable for a deployment whose stamps are known to be change times.
