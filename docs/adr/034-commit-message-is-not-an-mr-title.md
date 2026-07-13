# ADR 034: A commit message is not an MR title

## Status

Accepted (2026-07-13)

## Context

`update_file` optionally opens a merge request after committing. It built the MR
payload by passing the commit message straight through as the title:

```rust
"title": commit_message,
```

That works only for single-line commit messages. GitLab caps merge-request titles
at **255 characters** and rejects the entire create call with `400 Bad Request`
(`{"title":["is too long (maximum is 255 characters)"]}`) when the cap is
exceeded.

The failure mode is perverse: **the better the commit message, the more likely the
MR fails.** A one-line message works; a proper git message — subject, blank line,
explanatory body — reliably blows past 255 characters and loses the merge request.
The commit itself still lands, so the caller is left with a dangling branch and no
MR, which is exactly the state that is easy to lose track of in a batch.

It surfaced during a README documentation campaign: the commit explaining *why* a
README was being rewritten (stale toolchain claims, dead links to a predecessor
company's GitLab) ran to a dozen lines, and the MR silently did not get created.

There is also a latent panic here. Commit messages are not necessarily ASCII —
Cyrillic subjects are common in some of the repos this tool is pointed at.
Truncating with a byte slice (`&subject[..255]`) would sooner or later land in the
middle of a multi-byte codepoint and panic.

## Decision

Split the commit message the way git already defines it — subject, blank line,
body — and map it onto the two fields GitLab actually has:

- **subject → MR title**, truncated to 255 *characters* (with an ellipsis) if a
  caller writes an overlong subject line;
- **body → MR description**, where it was always meant to go.

Truncation counts `chars()`, not bytes, so Cyrillic subjects cannot panic.

If the subject is empty (a message beginning with a newline), `update_file`
backfills the title with `Update <file_path>` rather than sending a blank title,
which GitLab also rejects.

## Consequences

- Writing a real commit message no longer costs you the merge request. The two
  are no longer in tension.
- MR descriptions are populated for free from the commit body, so `update_file`
  MRs now arrive with their rationale attached instead of a bare title.
- Overlong or empty subjects degrade rather than fail.
- General lesson, and the second time this class of bug has bitten (see
  [ADR 033](033-archived-project-merge-status.md)): **the shape of our data is
  not the shape of the API's data.** A git commit message is structured
  (subject + body); a GitLab MR title is a bounded single line. Passing one where
  the other is expected type-checks fine and fails at runtime, on the inputs we
  most want to encourage.
