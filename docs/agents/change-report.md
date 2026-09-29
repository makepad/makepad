# Change reports: sending your changes to Makepad

People who install Makepad apps with the Makepad Builder change the apps'
source with their own coding agent (Codex desktop, Codex CLI, Claude Code or
any other). A change report tells Makepad what they changed, as concepts:
what is different for someone using the app, and why. The person reads the
report before it is sent; nothing leaves their machine without that.

This document is the format and the rules. `AGENTS.md` ("Sending your
changes to Makepad") tells an agent how to write one; `libs/change_report`
checks one exactly as described here, in the Builder before sending and on
the server when it arrives. The JSON schema of `report.json` is
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

The Builder can make the zip itself: an agent may leave the three files in
a folder, `builder/changes/<app>-report/`, instead of a zip at
`builder/changes/<app>-report.zip`.

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
| `app.id` | required | the app's id in the Builder (`scope`, `stage`, `calculator`, ...) |
| `app.release` | required | the release the edits were made against: `"release"` in `builder/installed/<app>.json` |
| `base` | optional | that release's repositories and commits (`"repositories"` in the same file), at most 8 |
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

Text fields hold no control characters other than newlines and tabs; titles
and areas are one line. Unknown fields are refused, so a report carries
nothing the person did not see listed.

## REPORT.md

The same changes, for people. It must name every change by its exact title;
otherwise its form is free. The Builder's own rendering, which agents may
copy:

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

The Builder and the server run a safety-net scan of every file and refuse a
report that still holds an email address (other than `@example.*` and
`@makepad.nl`), an absolute or home folder path, a well-known key or token
format (`sk-…`, `ghp_…`, `AKIA…`, `xoxb-…`, private key blocks, `Bearer …`,
`password = "…"`-style assignments), credentials in a URL, or a private
network address. The refusal names the file, line and kind of each finding.
Names of people and personal details in prose cannot be found this way:
they are the agent's to take out and the person's to check when reading.

## Sending

The person sends the report from the Builder (`e` on the app's row: the
review page lists the changes, then Send anonymously, Send with my email,
Read it all or Don't send), or the agent sends it after the person approved
the list in the conversation:

    makepad-builder.exe send-changes <app> [PATH] [--yes] [--with-email]   (Windows)
    ./makepad send-changes <app> [PATH] [--yes] [--with-email]             (macOS, Linux)

Without `--yes` it only checks the report and prints what would be sent.
`--with-email` includes the address the Builder is logged in with, so
Makepad can reply; the agent never sees it. A sent report is renamed
`<app>-report-sent-<id>` in `builder/changes/`.

`makepad-builder.exe changes <app>` / `./makepad changes <app>` prints the
edits to describe: the diff of the app's source snapshot against the
release it was checked out at (`--files` lists the files).

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

Without the Builder (a plain HTTP fallback):

    curl -sS -X POST -H 'X-Makepad-Feedback: 1' -H 'Content-Type: application/zip' \
         --data-binary @changes.zip https://makepad.nl/api/feedback/changes

The server keeps the zip as sent and REPORT.md as the text of a feedback
item in the admin Feedback inbox (source "changes"), with the release, the
change titles, what was taken out and whether a diff is included. No IP
address or other request data is stored.
