# pi-habit-hooks-rs (`pi-habit-hooks`)

High-performance native Rust core runtime and Pi/OMP extension for [Habit
Hooks](https://github.com/habit-hooks/habit-hooks) — a code-quality coaching
engine translating tool-specific lint violations into a canonical smell
vocabulary routed to outcome-oriented AI coaching prompts.

## Features

- **Native Rust CLI Runtime**: Zero-dependency standalone binaries
  (`habit-hooks`, `habit-sensors`, `habit-mapper`, `habit-snooze`).
- **Pi / OMP Extension**: Tools (`habit_hooks`, `habit_snooze`) and slash
  commands work on demand. Automatic firing is bounded and configurable, see
  [Controlling when hooks fire](#controlling-when-hooks-fire).
- **Declarative Rust Plugin**: Full support for `clippy`, `messrust`,
  `cargo-modules`, `cargo-machete`, and AST comment analysis.
- **Snooze Engine with Lapse Tracking**: Automatic exemption expiration when the
  affected file's Git blob SHA changes.
- **Shipped Generic Plugin**: Duplicate code detection powered by `jscpd` via
  local package dependency (no global npm required).

## Installation & Build

```bash
# Build native binaries and TypeScript extension
npm run build

# Run all test suites (Rust tests, drift gate, and Node extension tests)
npm test
```

## CLI Usage

```bash
# Initialize configuration in a project
habit-hooks init

# Run coaching checks on working tree changes
habit-hooks

# Run coaching checks on all files in the project
habit-hooks --all

# Snooze active findings
habit-sensors --all | habit-snooze --snooze --reason "Legacy code"

# List active snoozes with lapse status
habit-snooze --list
```

### Pre-commit hooks

`habit-hooks install` installs a pre-commit gate that skips commits without
staged Rust source files. In repositories with `hk.pkl`, it adds a Rust-scoped
Habit Hooks step and registers it with [hk](https://hk.jdx.dev/); `habit-hooks
install --hk` creates `hk.pkl` when needed. A native hook is used when hk is not
configured. Existing unrelated hooks are preserved rather than overwritten.

## Controlling when hooks fire

Each run does real work (`cargo clippy --all-targets` plus the other sensors),
so the extension fires on its own as rarely as it can and never blocks the host
while doing so.

| Mode | Fires automatically | Set with |
| --- | --- | --- |
| `off` | never; the tools and `/habit-hooks` still work when invoked | `/habit-hooks off` |
| `turn` (default) | once, when the main session tries to stop, and only if `.rs` files changed | `/habit-hooks turn` |
| `edit` | after every Rust edit (post-exec), plus the stop gate | `/habit-hooks edit` |

`/habit-hooks status` shows the current mode; `/habit-hooks` and `/habit-hooks
--all` run a check now. `/habit-hooks off|turn|edit` is saved to
`~/.config/habit-hooks/mode` (override the path with `HABIT_HOOKS_MODE_FILE`),
so it survives restarts, applies to every session, and also silences the
`habit-hooks --staged` pre-commit/hk step. Precedence: `HABIT_HOOKS_MODE` env,
then the file, then `turn`.

Guard rails, all on by default:

- **Async, bounded runs.** Checks run as child processes, never `spawnSync`, so
  the TUI and other agents stay responsive. Each run has a timeout
  (`HABIT_HOOKS_TIMEOUT_MS`, default 90000; edit scans use at most 30000) and a
  timeout kills the whole process group, including cargo and rustc.
- **One run per project at a time, shared by everyone.** Concurrent requests
  (stop gate, the `habit_hooks` tool, subagents) queue behind each other, and a
  finished run is reused until the set of changed `.rs` files, or their size or
  mtime, changes. The gate does not repeat a run the model just made through the
  tool.
- **The stop gate cannot trap the agent.** It refuses to let a session stop at
  most `HABIT_HOOKS_MAX_BLOCKS` times (default 2) per user prompt, then lets it
  finish and shows `(gate released)` in the status badge. Timeouts, crashes and
  other non-smell failures never block; a timeout also pauses automatic runs for
  that project for 5 minutes.
- **Main session only.** Subagents get no coaching prompt, no edit scans and no
  stop gate. The gate hooks `session_stop`, which hosts never fire for
  subagents; `turn_end`, which fires after every model turn and whose result
  hosts ignore, is not used.
- **Rust projects only.** Nothing fires without a `Cargo.toml`, a `.rs` file,
  and (for the gate) changed `.rs` files in git.
- **Per-agent isolation.** Every `edit`/`write`/`ast_edit` of a `.rs` file, and
  every shell command that changed one, is attributed to the agent whose tool
  call made it (subagents count toward their root agent). The stop gate, edit
  scans and `/habit-hooks` / `habit_hooks` (scope `working`) check only those
  files, so another agent's uncommitted edits never trigger, block or pollute
  your run. A file written while another agent's write-capable tool call
  overlapped (in this process, or in another `omp`/`pi` process on the same
  repo, tracked in `.habit-hooks/agents/<pid>.json`) is *soft*: still checked
  and shown, never enforced. If you wrote nothing, `habit_hooks` checks the
  whole working tree and tells you to pass `scope: "all"` for the full codebase.
  Attribution is stamped into tool-result `details.habitHooks` and rebuilt from
  the session branch on resume. `/habit-hooks status` lists your files, the soft
  ones, and the last gate decision. `/habit-hooks off|turn|edit --agent` changes
  the mode for this agent only (in memory, not saved); `/habit-hooks default
  possibly-writing shell call (two `git status` runs, 41 ms on a 5,000-file
  repo) and nothing for edits that name their file.
- **Blame comes from the CLI's report, not its text.** `habit-hooks --report
  <path>` writes JSON (`enforcedFiles`, `incompleteRun`, `smellCount`); the gate
  refuses only when an enforced file is one you wrote alone. A run with no report
  (timeout, older binary) never blocks. If the crate does not compile because of
  files outside the checked scope, `clippy_sensor` reports an `incomplete-run`
  finding instead of a false clean result; the gate shows `incomplete run (build
  broken outside your files)` and does not block on it.

Known limits: `clippy_sensor` still analyzes the whole crate and filters
findings to the requested files, so a first run in a project compiles clippy's
dependencies (hence the timeout); a shell command or other tool that writes
`.rs` files is only noticed for `bash`, `eval`, `python`, `edit`, `write` and
`ast_edit` (not `lsp` code actions or MCP tools); an overlapping write by
another agent makes attribution soft, so two agents running long commands at
once are not blocked on each other's smells nor on their own; cross-process
detection ignores processes whose window file is older than 10 minutes; runs
from separate `omp`/`pi` processes on the same repository are not otherwise
coordinated beyond cargo's own build-directory lock.
