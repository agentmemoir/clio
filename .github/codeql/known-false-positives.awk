# Drops a result whose rule id and file path both match the allowlist.
#
# Called as: awk -F'\t' -v lang=<language> -v keep=<file> -v drop=<file> -f known-false-positives.awk <allowlist> <findings.tsv>
# A kept row goes to keep, a dropped row goes to drop, and the kept count goes to stdout.
# FILENAME is compared instead of FNR == NR, because with a 0-byte allowlist FNR == NR is true for the first findings record and every result would be dropped.
BEGIN { OFS = "\t" }
FILENAME == ARGV[1] {
  line = $0
  sub(/#.*/, "", line)
  sub(/^[[:space:]]+/, "", line)
  n = split(line, entry, /[[:space:]]+/)
  if (n >= 2) known[entry[1] SUBSEP entry[2]] = 1
  next
}
{
  path = $3
  sub(/:[0-9]+$/, "", path)
  if (($2 SUBSEP path) in known) {
    print lang, $2, $3 >> drop
    next
  }
  print lang, $1, $2, $3, $4 >> keep
  kept++
}
END { print kept + 0 }
