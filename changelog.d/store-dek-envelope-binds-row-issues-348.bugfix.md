Item content is now sealed with its row id and subject id in the AEAD associated data, so a ciphertext copied from one row to another row of the same subject fails to open.

The row identity is the item, triple, task or failure, belief, persona `(bank, key)`, or sync mutation entity id, length-prefixed so two fields cannot run together.

A row sealed by an earlier release has no row binding and no read fallback, so it reports unreadable content; restore or re-import such content after upgrading.
