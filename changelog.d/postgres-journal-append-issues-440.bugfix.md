A concurrent Postgres journal append now reports the duplicate instead of failing, so a repeated push is acked as `Duplicate`.
