The dev container now runs as the non-root `ubuntu` user, so a process in the box no longer holds UID 0 or any capability, and git no longer reports dubious ownership on the read-only checkout.
