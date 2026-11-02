Item content is now sealed with its field role, row id, and subject id in the AEAD associated data, so a ciphertext copied from one item, triple, task, failure, or belief row to another row of the same subject fails to open; persona versions and sync journal rows are bound by their own identity instead.

The row identity is the item, triple, task, failure, and belief row id, the persona `(bank, table, key)` version, and the sync mutation id, length-prefixed so two fields cannot run together.

A row sealed by an earlier release has no field or row binding and no read fallback, so it reports unreadable content; restore it from a store written by this release, or re-import a `dsar_plaintext` export, which re-seals the content.
