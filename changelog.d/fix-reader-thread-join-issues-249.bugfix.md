The old-archive-restore test fixture now fails when one of its child-output reader threads panics, instead of discarding the join result and passing while a reader thread has already died.
