Item content is now sealed with its field role, row id, and subject id in the AEAD associated data, so a ciphertext copied from one item, triple, task, failure, or belief row to another row of the same subject fails to open; persona versions and sync journal rows are bound by their own identity instead.

The row identity is the item, triple, task, failure, and belief row id, the persona `(bank, table, key, version)` row including its stored row id, and the sync mutation id, length-prefixed so two fields cannot run together.

A value sealed by an earlier release has no field or row binding and no read fallback. Item content, triple objects, belief propositions, task definitions and steps, failure records, journal content, and persona stable values fail to open with an unreadable-content error. Sealed context columns (item, belief, persona stable, and persona preference) open tolerantly instead: the context becomes absent, the read still returns `Ok`, and a persona preference context disappears with no error. Restore the store from one written by this release, or re-import a `dsar_plaintext` export, which re-seals the content.

A `ciphertext_backup` import now refuses an opaque row unless the manifest names exactly the target bank, and a same-bank restore still works.
