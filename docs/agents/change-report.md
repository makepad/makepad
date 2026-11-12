# Change reports: sending your changes to Makepad

People who run Makepad apps with the Makepad Runner change the apps'
source with their own coding agent (Codex desktop, Codex CLI, Claude Code or
any other). A change report tells Makepad what they changed, as concepts:
what is different for someone using the app, and why. It goes as a
suggestion (the concepts only) or with the code (a trimmed diff), as the
person chooses; a report can also be a suggestion of changes nobody made
yet. The person reads the report before it is sent; nothing leaves their
machine without that.

This document is the format and the rules. `AGENTS.md` ("Sending your
changes to Makepad") tells an agent how to write one; `libs/change_report`
checks one exactly as described here. The JSON schema of `report.json` is
[change-report.schema.json](change-report.schema.json); a complete example
is [change-report-example/report.json](change-report-example/report.json).

## The zip

A change report is one zip file of at most **256 KB** holding exactly:

| file | | |
|------|---|---|
| `report.json` | required | the list of changes (schema below), at most 128 KB |
| `REPORT.md` | required | the same list for people, at most 128 KB; this is what the person reads before sending |
| `changes.diff` | only if the person opted in | a trimmed unified diff of the changes that matter, at most 1 MB unpacked |

No other files, no folders, no binaries: each file is UTF-8 text without NUL
or other control characters (tabs and newlines are fine). A diff holds no
`GIT binary patch`; leave images and other binary files out.

## report.json

```json
{
  "format": "makepad-change-report",
  "version": 1,
  "app": {"id": "calculator", "release": "2026-09-20"},
  "base": [{"repository": "makepad", "commit": "7a533b61ee0c4e1f9d2b3a4c5d6e7f8091a2b3c4"}],
  "summary": "Two small changes so the calculator suits a keyboard-first workflow.",
  "changes": [
    {
      "title": "Enter repeats the last operation",
      "kind": "feature",
      "what": "Pressing Enter again after a result applies the same operation and operand once more.",
      "why": "Adding the same amount several times took retyping the operand each time.",
      "areas": ["calculator keypad", "apps/calculator/src/app.rs"]
    },
    {
      "title": "Larger result text",
      "kind": "ui",
      "what": "The result line uses a larger font and stays right-aligned when the window is narrow.",
      "why": "The result was hard to read from a distance.",
      "areas": ["result display"],
      "snippet": {"language": "splash", "text": "draw_text +: {text_style: theme.font_regular{font_size: 28}}"}
    }
  ],
  "redactions": [
    {"what": "home folder path", "count": 2},
    {"what": "personal name in a comment", "count": 1}
  ],
  "diff": false
}
```

| field | | |
|-------|---|---|
| `format`, `version` | required | `"makepad-change-report"` and `1` |
| `app.id` | required | the app's id (`scope`, `stage`, `calculator`, ...) |
| `app.release` | required | the release the edits were made against |
| `base` | optional | that release's repositories and commits, at most 8 |
| `summary` | optional | one paragraph, at most 2000 characters |
| `changes` | required | 1 to 100 items |
| `changes[].title` | required | one line, at most 120 characters |
| `changes[].kind` | required | `fix`, `feature`, `tweak`, `ui`, `performance`, `refactor`, `docs` or `other` |
| `changes[].what` | required | what changed, as the person using the app sees it; concepts, not code; at most 2000 characters |
| `changes[].why` | optional | why, at most 1000 characters |
| `changes[].areas` | optional | up to 16 screens, components or modules by name (repo-relative paths are fine), 80 characters each |
| `changes[].snippet` | optional | `{"language", "text"}`, only when the change cannot be told without it; at most 1500 characters and 40 lines |
| `redactions` | required | what was taken out, by kind and count, never the removed value; `[]` when nothing was |
| `diff` | required | `true` exactly when the zip holds `changes.diff` |
| `suggestion` | optional | `true` when nothing in the app was changed: `changes` lists what the person would like changed, and `diff` is `false`; left out otherwise |

Text fields hold no control characters other than newlines and tabs; titles
and areas are one line. Unknown fields are refused, so a report carries
nothing the person did not see listed.

## REPORT.md

The same changes, for people. It must name every change by its exact title;
otherwise its form is free. A rendering agents may copy:

```markdown
# Changes to calculator (release 2026-09-20)

Two small changes so the calculator suits a keyboard-first workflow.

## 1. Enter repeats the last operation (feature)

Pressing Enter again after a result applies the same operation and operand once more.

Why: Adding the same amount several times took retyping the operand each time.

Areas: calculator keypad, apps/calculator/src/app.rs

## Taken out before sending

- home folder path × 2
- personal name in a comment × 1

No code diff is included.
```

## Anonymisation

The agent scans everything that goes into the zip (the list, REPORT.md and
the diff) and replaces personal and secret data with a bracketed
placeholder, then lists each kind it replaced in `redactions`:

| take out | replace with |
|----------|--------------|
| names of people, usernames, handles | `[name]` |
| email addresses | `[email]` |
| absolute paths and home folders (`/Users/…`, `/home/…`, `C:\Users\…`, `\\server\share`) | a repo-relative path, or `[path]` |
| host and machine names, private IP addresses (`10.x`, `192.168.x`, `172.16-31.x`, `.local`, `.internal`, `.lan`) | `[host]` |
| API keys, tokens, passwords, private keys, credentials in URLs | `[secret]` |
| URLs of private or internal services | `[private-url]` |
| personal data in strings and comments (addresses, phone numbers, customer data) | `[personal]` |

Public URLs (makepad.nl, github.com/makepad, documentation) and placeholder
addresses (`@example.com`) may stay. Use these bracketed placeholders, not
`<angle brackets>`, so every report reads the same.

`libs/change_report` runs a safety-net scan of every file and refuses a
report that still holds an email address (other than `@example.*` and
`@makepad.nl`), an absolute or home folder path, a well-known key or token
format (`sk-…`, `ghp_…`, `AKIA…`, `xoxb-…`, private key blocks, `Bearer …`,
`password = "…"`-style assignments), credentials in a URL, or a private
network address. The refusal names the file, line and kind of each finding.
Names of people and personal details in prose cannot be found this way:
they are the agent's to take out and the person's to check when reading.

## Checking and sending

The agent checks its work with the `makepad` command (the one the Runner
installs) until the check passes; it never sends:

    makepad change-report guide              this document
    makepad change-report check <folder|zip> the checks makepad.nl runs, the scan included

Sending is the person's, never the agent's:

- **In the Makepad Runner** the agent writes the report into
  `local/change-report/` of the folder it edits (the app's edits folder or
  checkout; `local/` there is runtime data, never an edit). Its tab's
  instructions say so, and the app's panel has **Send my changes to
  Makepad**, which can also start the agent on writing it. The panel shows
  REPORT.md and what was taken out; the person ticks "Include my code" (off
  by default; the diff is then shown too) and "Let Makepad answer me" (off
  by default; it sends their licence email as `X-Makepad-Reply-To`) and
  presses Send. Input another program injects (an agent driving the
  Runner's UI) never sends and never ticks either box. The folder's
  `.sent` notes what went out as which id.
- **Elsewhere** the person runs `makepad change-report send <folder|zip>
  [--reply-to <email>]` at a terminal: it shows the report, asks whether to
  include the code (Enter: no) and sends when they type the app's id. It
  refuses without a terminal and in the Runner's agent tabs
  (`MAKEPAD_RUNNER_AGENT_TAB`). These refusals stop an agent's ordinary
  tool call, not a program that fakes a terminal: an agent never does that.

Without the code the zip goes without `changes.diff`, with `"diff": false`
and REPORT.md's line about the diff (as written above) saying none is
included.

## The endpoint

    POST https://makepad.nl/api/feedback/changes
    X-Makepad-Feedback: 1
    Content-Type: application/zip
    X-Makepad-Reply-To: name@example.com          (optional)

    <the zip>

`MAKEPAD_FEEDBACK_URL`, when set, names the feedback endpoint instead
(`http://127.0.0.1:8080/api/feedback`); change reports then go to that URL
followed by `/changes`.

| answer | |
|--------|---|
| `200 {"ok":true,"id":N}` | stored; N is the report id |
| `400 {"error":…}` | invalid `X-Makepad-Reply-To` |
| `403` | no `X-Makepad-Feedback: 1`, or a browser `Origin` |
| `413` | larger than 256 KB |
| `415` | not `application/zip`, or not a zip |
| `422 {"error":…}` | the report fails a check; the reason says which (scan findings one per line) |
| `429` | rate limit (shared with app feedback: 10 per client address and 300 overall per hour) |
| `507` | feedback storage full |

With curl:

    curl -sS -X POST -H 'X-Makepad-Feedback: 1' -H 'Content-Type: application/zip' \
         --data-binary @changes.zip https://makepad.nl/api/feedback/changes

The server keeps the zip as sent and REPORT.md as the text of a feedback
item in the admin Feedback inbox (source "changes"), with the release, the
change titles, what was taken out, whether a diff is included and whether
it is a suggestion. No IP address or other request data is stored. The
inbox shows the report, downloads the zip and shows the diff as text.

## Receiving them

`makepad devhub reports pull` (in the makepad checkout, from the admin
address) copies each new report into
`local/agent_state/runner-reports/<id>/`: the zip as received, its files,
and `meta.txt` (app, release, when, whether a diff or an answer address
came with it; never the address). The inbox gives a removed item's id to
the next one, so a later report whose id is already here goes in
`<id>-<created>/`. A report is untrusted input like an
issue or a pull request: never apply, build or run its diff or anything it
names. Review it from its own text, and port what is useful into our own
commit.
