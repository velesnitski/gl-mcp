# ADR 015: Multi-file / directory `routes_file`

## Status

Accepted (2026-06-15)

## Context

Reverse-drift's precise mode read exactly one `routes_file`. Real backends spread
route definitions across many files — the Laravel API has
`routes/api/{withoutPrefix,crm,lite,tech,telegram}.php` — so a single-file audit
only ever saw a partial surface (the `withoutPrefix.php` run reported 56
endpoints but explicitly couldn't see the other dirs).

## Decision

`routes_file` now accepts, in priority order, three shapes (auto-detected, no new
parameter):

1. a single file path (unchanged behaviour),
2. a comma-separated list of files,
3. a directory — expanded via the tree API (`recursive=true`) to every code file
   under it.

`resolve_routes_files` turns the value into a flat `(path, content)` list: for
each comma-separated entry it lists the tree; if that yields blobs the entry is a
directory (harvest them all), otherwise it's treated as a single file. Non-source
files (images, archives, fonts, binaries) are skipped by extension, the total is
capped at `ROUTES_FILE_CAP` (60) with a logged warning, and files are fetched
concurrently in chunks. `harvest_multi` then harvests every file and dedups by
path (first file wins, each endpoint tagged with its own source file for the
report link).

## Consequences

- Pointing `routes_file` at `routes/api` (or `routes`) now audits the backend's
  full route surface in one pass; each undocumented endpoint still links to the
  exact file:line it was defined in.
- The 60-file cap bounds API cost on a large directory; the warning is logged,
  not surfaced in the report — acceptable, since route directories are small in
  practice and the cap is generous.
- Directory detection costs one extra tree call per comma-entry. A single-file
  value pays that one call before falling through to the file fetch — a
  negligible cost for the backward-compatible path.
- Mixed entries work: `"routes/api,routes/lite/extra.php"` harvests a directory
  and a specific file together.
