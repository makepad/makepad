# Makepad Code

Makepad Code is a local coding agent in a native window. It talks to a model
on your own AI Hub (or this machine's weights), works inside one folder, and
shows what it does: every file it reads, every command it runs, every edit as
a diff you can accept hunk by hunk.

```sh
cargo run --release -p makepad-app-code -- ~/my-project
cargo run --release -p makepad-app-code -- ~/my-project --approve ask --context 32768
```

- The folder is the sandbox: file tools cannot leave it. Approval modes are
  `ask` (everything asks), `allowlist` (reads and safe commands run, the rest
  ask; the default) and `auto`.
- Edits show their diff before anything is written. Tick hunks with 1-9,
  `y` applies the ticked ones, `a` accepts all, `n` rejects.
- `run_command` output streams into a terminal block while it runs; `Esc`
  interrupts.
- A plan panel follows the model's todo list; sub-agents show as lanes.
- Sessions are saved under `~/.makepad/code/sessions` and listed in the left
  rail; click one to resume it. Old turns fold into a summary as the context
  fills (the heat strip in the status bar shows what fills it).
- The file tree lists lazily; the editor pane (`Cmd/Ctrl+E`) follows the files
  the agent reads and edits and opens the ones you click.
- `Cmd/Ctrl+K` opens the command palette: new session, resume, approval mode,
  context size, panes, dark or light theme, interrupt, copy last answer.

## In a terminal

`makepad-code` is the same agent without a window: a plain binary (no GPU, no
display) for ssh sessions, a bare console, or a machine whose desktop is down.

```sh
cargo build --release -p makepad-code
makepad-code                       # in the project folder, or: makepad-code ~/my-project
makepad-code --approve ask --context 32k --resume last
makepad-code -p "list the files here and count lines in README"   # one task, then exit
```

- The prompt sits at the bottom; the conversation scrolls above it in the
  terminal's own scrollback. Answers stream in; each tool is one row (state,
  title, summary); `run_command` output streams under its row, dim, and folds
  to its last lines when done; edits show a coloured diff.
- Approvals ask inline: `[y] allow  [a] this session  [r] this repo  [n] deny`
  for commands; for edits `1-9` tick hunks, `y` applies the ticked ones, `a`
  all, `n` rejects. Questions from the agent get an answer prompt.
- `Esc` or `Ctrl+C` interrupts the current step; `Ctrl+D`, or `Ctrl+C` twice at
  an empty prompt, exits. `Alt+Enter` or a trailing `\` adds a line; pastes stay
  one prompt; Up/Down walk the history (`~/.makepad/code/cli-history`).
- `/new`, `/resume [N|ID]`, `/sessions`, `/model [32k | kv q8|f16]`,
  `/approve ask|allowlist|auto`, `/ref PATH`, `/clear`, `/help`, `/exit`.
  `@path` in a request attaches that file; `Tab` completes it from the repo.
- Sessions are the window's (`~/.makepad/code/sessions`): one started in the
  window resumes here, and back. The model is elected as in the window: this
  machine's hub, then a LAN node.
- Without a terminal (pipes, scripts) it reads lines from stdin: requests,
  then answers to approvals. `NO_COLOR` or a pipe gives plain text;
  `TERM=linux` or a non-UTF-8 locale gives ASCII marks.

Layout: `agent/` (crate `makepad-code-agent`) is the core (no UI types:
tools, approvals, context, sessions, sub-agents, the file index), `src/ui` and
`src/ide` the window, `cli/` (crate `makepad-code`) the terminal.
`CODE_MOCK=1` runs either front end against a scripted agent without a model.
