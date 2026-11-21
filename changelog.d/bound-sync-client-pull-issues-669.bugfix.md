A sync client pull is now bounded instead of trusting the peer to stop it. Before this change the pull loop fetched page after page as long as the server answered `has_more`, so a misbehaving or hostile peer could keep one pull running without end. The pull now stops with an error on each of these bounds:
- 1000 pages for one pull of one bank.
- A page that hands back a cursor this pull already used, whether that is the cursor the page was asked from or any cursor an earlier page of the same pull handed back: a peer that reports more rows but does not advance the cursor, or that flips back and forth between two cursors, would otherwise serve the same pages forever.
- A page response larger than 32 MiB.
- A cursor token larger than 4 KiB.
A page that is applied persists its cursor, so a legitimately larger backlog resumes from the stored cursor on the next pull; a refused page, a frozen page, and the cap exit store nothing new.
