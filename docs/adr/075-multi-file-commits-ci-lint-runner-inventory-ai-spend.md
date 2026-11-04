# ADR 075: Multi-file commits, CI lint, runner inventory, AI spend in the adoption report

## Status

Accepted (2026-11-04)

## Context

Running scheduled reports on a GitLab instance through this server showed four gaps
that each ended in a hand-written script:

1. **One file per commit.** `update_file` writes one file. A change to a pipeline
   and the script it calls became two commits and two merge requests, or a script
   against the commits API.
2. **No way to check CI configuration before merging.** A YAML mistake (an unquoted
   `KEY: value` inside a script line parses as a mapping) is only discovered when
   the merged pipeline fails with zero jobs.
3. **No instance-wide runner view.** Runners on an unsupported major version, idle
   or paused runners, and which machine runs what, needed several admin endpoints
   joined by hand.
4. **No cost next to adoption.** The adoption report shows how much AI is used, not
   what it costs. Commits can carry their own spend as trailers (`AI-Model`,
   `AI-Tokens`, `AI-Cost`, the convention of the open-source claude-token-trailers
   plugin), so the cost can be read from the history the report already scans.

Two smaller defects surfaced at the same time: `create_merge_request` reported
"0 files" right after creation (GitLab fills the MR diff asynchronously), and
`update_pipeline_schedule` headed its one-row answer with a count that read as the
project's total.

## Decision

1. **`commit_files`** (write tool): several files created, updated or deleted in one
   commit, with the rules of `update_file` (feature branch only, never a protected
   branch; optional MR). Create vs update is decided per file. When `.gitlab-ci.yml`
   is among the files, the new branch's configuration is linted and the verdict is
   part of the answer.
2. **`lint_ci_config`**: the CI Lint result for a ref, or for YAML passed in before
   it is committed. Errors are listed verbatim; the YAML-mapping error gets a
   one-line hint.
3. **`list_runners`**: every runner on the instance with an admin token (the
   token's own runners otherwise, said so), one row per runner machine: version and
   its lag behind the instance (an older major is flagged as unsupported), state
   (paused counts as such), platform, last contact, tags, scope; optionally jobs in
   the last 30 days. Oldest versions first. No addresses are shown.
4. **AI spend in the adoption report**: the commits the scan already fetches are
   read for `AI-Cost` / `AI-Tokens` trailers and summed per repository. An "AI
   Spend" section (HTML and markdown) appears only when some repository stamps its
   commits. It is labelled as API list price and as a per-change reading, not a
   per-person target. No extra API calls.
5. **Fixes:** MR diff stats come from the compare endpoint (computed on request) and
   say "no changes" when there are none; the schedule update answer shows just the
   updated row.

## Consequences

- Spend appears only for repositories whose commits carry the trailers; teams that
  keep spend in git notes, or not at all, are absent rather than shown as zero.
- `list_runners` makes two or three calls per runner; it is capped at 200 runners.
- `commit_files` reads each file's existence once per file before committing.
