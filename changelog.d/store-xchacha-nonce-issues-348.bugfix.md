Item content is now sealed with XChaCha20-Poly1305 and a random 24-byte nonce, so a content key is never reused under a colliding random nonce even after a very large number of writes per subject.

An item sealed by an earlier release carries a 12-byte nonce, and the current cipher cannot open it: such a row reports unreadable content instead of plaintext, so a store written before this upgrade must be restored from a store written by this release, or re-imported from a `dsar_plaintext` export.
