`clio ops reindex` now reports two new counters, `unreadable` and `erased`, so a row the index cannot derive from no longer disappears without a signal. `unreadable` counts content that will not open: a damaged ciphertext, a missing content column, an old opaque envelope, or a content version this build cannot read. `erased` counts a crypto-shredded subject, which is an expected loss and stays separate from damage.

The rebuild sweep and the outbox worker fill the counters at the point where the read error is already in hand. A lost row is still finished, never retried, and a backend failure still fails closed, so the counters change no control flow.

A damaged row counts once per rebuild, even when the run refreshes both the dense and the lexical leg. An old row that used to be reported as a worker skip now shows in `unreadable`, so a `skipped` count of zero no longer hides it.
