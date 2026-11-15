# Working notes

This directory holds open questions, TODO lists, capture requests,
investigations and handovers. It is not part of the specification; the
authoritative model lives in `docs/spec/`.

## Sharing rule

- Work notes are local by default. A local `.git/info/exclude` keeps new
  files in this directory out of commits.
- A note is shared only by an explicit decision: `git add -f
  docs/work/<note>.md`. The explicit add is the sharing decision.
- Share a note when other contributors need it to continue the work: a
  component backlog, an open-item list, a capture request the
  specifications rely on, or the investigation behind a specification
  decision.
- Personal notes, scratch and drafts stay local.
- `git ls-files docs/work/` shows the shared notes.
- Committed documents must not reference local-only notes. Anything a
  specification relies on belongs in the specification itself or in a
  shared note.

## Content rules

- The root `AGENTS.md` rules apply here too: no personal identifying
  information, no serial numbers, USB vendor/product IDs, hostnames,
  usernames or home-directory paths; no captured device state.
- Distinguish verified from assumed at every step. Name the capture or
  test behind a verified claim; label the rest as hypothesis and say
  what evidence would decide it.
- Backlog items carry a status: open, needs-capture, optional, done.
- One idea per sentence.