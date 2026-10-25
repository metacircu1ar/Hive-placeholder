# Agents in your terminal

Watch agents work and talk to them from a terminal. Pick the view that fits:

| Command | Shows | Runs in |
| --- | --- | --- |
| `hivectl tui` | Every agent: a tree sidebar, tabs and side-by-side chat views, questions waiting for you, a spawn dialog | Any terminal, including inside zellij and tmux |
| `hivectl attach AGENT_ID` | One agent as a full-screen chat | Any terminal |
| `hivectl panes` | One native pane per agent, opened and closed with the agents | zellij or tmux (macOS, Linux), or Ghostty 1.3+ on macOS |
| `hivectl live AGENT_ID --follow` | Streamed text and tool output as JSON lines, for scripts | Anywhere |

```sh
hivectl daemon start                                   # views never start the daemon
hivectl spawn implementor --cwd "$PWD" --body "Add a --dry-run flag to the deploy script."
hivectl tui
```

Closing a view never stops an agent. Every view reconnects by itself after a
daemon restart, and exits within a second when its terminal closes (a closed
window, a killed multiplexer session or a dropped SSH connection).

## Contents

- [hivectl tui](#hivectl-tui)
- [hivectl attach](#hivectl-attach)
- [hivectl panes](#hivectl-panes)
- [Supported terminals](#supported-terminals)
- [Limits and troubleshooting](#limits-and-troubleshooting)

## hivectl tui

```sh
hivectl tui                                            # every live agent
hivectl tui --root AGENT_ID --max-views 4 --keep-ended --quiet
```

The screen has five parts:
- **Sidebar** (left): the agent tree. Children sit under their parent, each with
  a badge. An agent shows its [name](cli-reference.md#agent-names), or its role
  and short ID; tabs and split titles name it the same way.
- **Tab bar** (top): numbered tabs, each holding one view or two side by side.
- **Views**: `hivectl attach` chats. The active view has a yellow border and takes
  your typing.
- **Status line** (bottom): messages, questions waiting for you, and connection
  problems.
- **Button bar** (last line): Sidebar, Split, Close, Questions, Spawn and Quit,
  to click instead of using the prefix keys.

Start in the sidebar: click an agent, or type part of a name, role, ID or state
to filter and press Enter.

### Badges

| Badge | Meaning |
| --- | --- |
| `!` | Needs you: a question or approval is waiting |
| `●` | Working |
| `◔` | Waiting for quota |
| `○` | Idle |
| `✓` | Done (with `--keep-ended`) |
| `✗` | Failed |
| `–` | Cancelled or ended (with `--keep-ended`) |

A collapsed parent shows how many agents it hides (`+3`) and their strongest
badge: needs you, then working, then quota.

### Sidebar keys

| Key | Effect |
| --- | --- |
| Letters, digits | Filter by name, role, short ID, state or badge name |
| Backspace | Edit the filter |
| ↑ ↓, Home, End | Move the selection |
| Enter | Open the agent, or focus the view it already has |
| ← → | Collapse or expand a parent (← on a child jumps to its parent) |
| Esc | Clear the filter and return to the active view |

### Prefix keys

Everything else starts with the prefix, **Ctrl-A** by default, followed by one
key (or click the button bar):

| After Ctrl-A | Effect |
| --- | --- |
| `s` | Focus the sidebar |
| `t` | Open the next agent you pick in the sidebar in a new tab |
| `v` | Split the tab left/right with the next agent you pick |
| `h` | Split the tab top/bottom with the next agent you pick |
| `o` | Switch to the other view of a split |
| `w` | Close the current view; the agent keeps running |
| `1`–`9` | Go to tab N |
| `n`, `p` | Next or previous tab |
| `a` | Questions waiting for you |
| `c` | Spawn an agent |
| `q` | Quit |
| Ctrl-A | Send Ctrl-A itself to the view |

The status line shows a reminder while the prefix waits. A prefix expires after
1.5 s, and an unbound key after it is dropped rather than typed.

**Ctrl-C** quits from anywhere. If a view still has a message, answer or command
in flight, the first Ctrl-C (or prefix `w`/`q`) only warns; press it again to
go ahead.

### Views

The active view takes every [`hivectl attach` key](#keys) and
[command](#commands). In `hivectl tui`:
- `/quit` closes the view.
- `/attach ID` opens or focuses that agent's view.
- An agent that ends keeps its view for 3 s, then the view closes. With
  `--keep-ended`, the view stays open and the agent stays in the sidebar with
  its outcome.

### Questions waiting for you

Prefix `a` lists every agent with a pending question or approval, oldest first
(the order this `hivectl tui` first saw them). Enter opens the agent with its
question selected, ready to answer. ↑ ↓ move through the list; Esc closes it.

A new question rings the terminal bell and shows the notification "Hive: N
agents need you". `--quiet` turns both off. Ghostty, iTerm2 and WezTerm show the
notification; Terminal.app does not. zellij and tmux do not pass it on, and
whether they pass the bell depends on their settings.

### Spawning

Prefix `c` opens the spawn dialog:

| Field | Value |
| --- | --- |
| Role | A configured role, chosen with ← → |
| Name | Typed and optional; a label for people, not unique |
| Login | A login profile or login group, chosen with ← →; empty uses the role's or the default |
| Model, Effort | Typed; empty uses the role's or the login's default. When [`tui.toml`](#model-and-effort-lists) lists names for the provider, ← → cycle through them and the default, replacing the text |
| Sandbox | ← → choose `read-only`, `workspace-write`, `full-access`, or the role/default. Full-access runs as your user without native filesystem/network confinement. |
| Directory | The agent's working directory; the current one by default |
| Task | The first message; Ctrl-J starts a new line |

The dialog shows the provider the choice resolves to. Tab and ↑ ↓ move between
fields, Enter spawns and Esc cancels. Problems the daemon finds appear under the
field they concern, for example an unknown model. The new agent opens in its own
tab.

Full-access agents show a red `full-access` badge in the tree and chat header.
Human dialog spawns may choose any sandbox. Agents using Hive tools default to
equal or narrower modes; `runtime.allow_spawn_widening: true` permits wider
children. See the [sandbox reference](cli-reference.md#spawn-and-branch-agents).

A dialog spawn has no parent. Under `--root`, it starts outside the TUI's scope;
reopen it later with `hivectl attach ID`.

### Mouse

| Where | Click | Wheel |
| --- | --- | --- |
| Sidebar row | Open the agent, or focus the view it has | Move the selection |
| `▸` / `▾` marker | Collapse or expand the subtree | Move the selection |
| Tab bar | Go to that tab | — |
| A view | Make it the active view; focus its input, question or transcript | Scroll its transcript, without changing the active view |
| Questions list | Open that agent at its question; a click outside closes the list | Move the selection |
| Spawn dialog | Focus a field; on a choice, `‹` picks the previous value and `›` the next | — |
| Button bar | Sidebar, Split, Close, Questions, Spawn or Quit, as the prefix keys do | — |

While the mouse works in the TUI, hold Shift to select text with the terminal
(Option in iTerm2). `--no-mouse`, or `mouse = false` in `tui.toml`, leaves the
mouse to the terminal. Inside tmux, clicks reach the TUI only after
`set -g mouse on`; zellij passes them on by default.

### Options

| Option | Effect |
| --- | --- |
| `--root AGENT_ID` | Only this agent and its descendants |
| `--max-views N` | Most views open at once, 1–12 (default 6) |
| `--keep-ended` | Keep ended agents in the sidebar and their views open |
| `--quiet` | No bell or notification for new questions |
| `--no-mouse` | Leave clicks and the wheel to the terminal |

### Settings file

Write `tui.toml` beside your configuration file: `~/.config/hive/tui.toml` by
default, or the directory of `--config`.

```toml
prefix = "ctrl-b"   # the prefix key (default "ctrl-a")
mouse = false       # leave the mouse to the terminal (default true)
```

A prefix is `ctrl-` followed by a letter or one of `]`, `\`, `^` and `_`.
Ctrl-C, Ctrl-H, Ctrl-I, Ctrl-J, Ctrl-M and Ctrl-R are refused: views use them,
or terminals cannot tell them from Backspace, Tab or Enter. Neither zellij nor
tmux binds Ctrl-A by default; inside screen, which does, choose another prefix. An unreadable file exits with
`config_unavailable` and an invalid one with `config_invalid`. Only `hivectl tui`
reads this file.

#### Model and effort lists

List the models and efforts you switch between, per provider:

```toml
[codex]
models = ["gpt-6-luna", "gpt-6"]
efforts = ["low", "medium", "high"]

[claude]
models = ["opus", "sonnet"]
efforts = ["low", "medium", "high", "max"]
```

`/model` or `/effort` without a name then opens a picker of the agent's
provider's names, and the spawn dialog's Model and Effort fields cycle through
them with ← →. You can still type any name: the lists are only shortcuts, and
the provider decides what it accepts.

- Each list holds at most 32 distinct names. Models use the `hivectl spawn --model`
  rule: 1–128 ASCII letters, digits or `-_.:@/[]`, not starting with `-`.
  Efforts are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` or
  `ultra`.
- Either table and either list can be left out; an empty list is the same as
  none.
- `hivectl attach` and `hivectl panes` views do not read this file. There, type
  `/model NAME` or `/effort LEVEL`.

## hivectl attach

```sh
hivectl attach AGENT_ID                                # chat with one agent
hivectl attach AGENT_ID --read-only                    # watch only
hivectl attach AGENT_ID --exit-on-end --linger 10      # leave 10 s after it ends
hivectl attach AGENT_ID --no-mouse                     # leave the mouse to the terminal
```

The view shows:
- a header with the agent's name (when it has one) and ID, state, model and
  effort, login, parent, children and queue;
- the transcript: messages with their sender, Markdown replies, tool calls with
  their last output lines, turn markers, steering and login notices, and
  questions;
- the running turn, streaming below the stored history;
- an input line and a key reminder.

### Keys

| Key | Effect |
| --- | --- |
| Enter | Send. While a turn runs, steers it if the agent supports steering; otherwise queues |
| Alt-Enter | Queue for the next turn |
| Ctrl-R | Redirect: interrupt the turn, then deliver this text first |
| Esc Esc | Interrupt the running turn |
| Ctrl-J | New line in the input |
| ← →, Home, End, Backspace, Delete | Edit the input |
| ↑ ↓ | Scroll by a line |
| PgUp, PgDn | Scroll by a page; scrolling past the top loads older history |
| Tab | Complete a view command after `/`; otherwise move focus: input, waiting question, transcript |
| Ctrl-C | Leave; asks again while a write is still in flight |
| Click | Focus the input, the waiting question or the transcript; on a question, its Accept, Decline and Cancel buttons answer it, and a click on a form field selects it or cycles its choice |
| Wheel | Scroll the transcript three lines a step |

With the mouse on, hold Shift to select text with the terminal (Option in
iTerm2); `--no-mouse` leaves the mouse to the terminal.

### Commands

Type these into the input:

| Command | Effect |
| --- | --- |
| `/name NAME` | Rename the agent at once; names are labels and need not be unique |
| `/model NAME`, `/effort LEVEL` | Change the model or effort from the next turn. Without a name, `hivectl tui` opens a picker of the [listed names](#model-and-effort-lists) |
| `/options KEY=VALUE ...` | Change other agent options |
| `/auth NAME` | Change the login profile or login group |
| `/stop`, `/resume` | Stop the agent, keeping its conversation; resume it later |
| `/interrupt` | Interrupt the running turn |
| `/terminate` | Cancel the agent permanently |
| `/complete TEXT` | Mark the task complete with this result |
| `/branch TEXT` | Start a new agent from this conversation |
| `/spawn ROLE TEXT` | Start a new root agent and show its ID |
| `/attach ID` | Switch this view to another agent |
| `/accept`, `/decline`, `/cancel` | Answer the waiting question |
| `/usage` | Show this agent's login usage: 5-hour and weekly limits, Codex credits, Claude extra usage |
| `/id` | Show the agent's full ID and copy it to the clipboard |
| `/help` | List these commands |
| `/quit` | Leave |

`/id` copies through the terminal's clipboard escape (OSC 52). Most terminals
accept it, with two setups to note:
- iTerm2 needs "Applications in terminal may access clipboard" (Settings →
  General → Selection).
- tmux needs `set -g set-clipboard on`.

Terminal.app does not support it. The status line always shows the full ID as
well.

Tab completes a view command once you type `/`. The input's top border lists the
commands that match, and shows a command's arguments once its name is complete.
With several matches, Tab fills in their common start, then cycles through them.
A command Hive does not know is left as typed and is sent to the agent.

The picker takes the question card's place: ↑ ↓ move, Enter or a click sets
the name, and Esc closes it. `✓` marks the agent's current value. A question
that arrives meanwhile waits under the picker, and the status line says so.

Any other `/text`, such as `/review src/`, is sent to the agent as an ordinary
message, and the status line says it was forwarded. The agent's model reads
it as text: Hive does not run the provider's own slash commands. A Claude agent
with `claude.skills` (see [Provider skills](cli-reference.md#provider-skills))
can use its bundled skills when asked, in plain words ("use the code-review
skill on the current diff") or with a forwarded `/code-review`; the model
decides to call the skill.
Start a message with `//` to send a line that begins with a view command's
name: `//model is too slow` sends `/model is too slow`. A view command with the
wrong arguments is never sent; the status line shows its usage instead.

`/usage` shows what the provider last reported for the agent's login, for
example:

```text
codex-main: 5h 12% used, resets in 2h 38m · weekly 30% used, resets in 5d 18h · credits 120.00 · reported 3 min ago
```

Providers report usage during turns (Codex also when its process starts), so a
login shows nothing until its first turn after the daemon starts. `hivectl auth
usage` prints every login's usage as JSON.

### Questions

A waiting question appears as a card above the input, with Accept, Decline
and Cancel buttons on its last line; Tab focuses it, or click it. Long lines
wrap, and the card grows up to half the view. A question taller than that keeps
its title and marks skipped rows with `…`. It shows its end, or the rows from the
selected field when that field is further up.
- **Simple approval.** `a` accepts, `d` declines, `c` cancels.
- **Form.** ↑ ↓ select a field, ← → choose a value, and you type into text and
  number fields. Enter accepts once the answer passes the question's checks;
  a problem shows on the status line.
- **Array fields.** Type or paste a JSON array, such as `["a","b"]`. The
  question's allowed values and minimum/maximum item counts still apply. Leave
  an optional array blank to omit it, or enter `[]` for an empty array.

Large transcript entries are loaded from their saved output on a background
thread. Each history page loads at most 512 KiB of this output. Larger or
unavailable entries show an artifact reference and the `hivectl output ID` command
to read it; any text already streamed into the view stays visible.

Every write is sent once, with its own request ID. A retry after a lost reply
reuses that ID, so the daemon answers it instead of acting twice.

## hivectl panes

```sh
hivectl panes                                          # every live agent
hivectl panes --root AGENT_ID --max-panes 8 --linger 10
hivectl panes --backend tmux                           # choose the backend explicitly
```

Run `hivectl panes` in one pane or tab. It keeps one `hivectl attach` view open for
each live agent and closes it once the agent has ended and the linger (default
3 s) has passed. Leaving the controller never closes panes or stops agents. A
restarted controller adopts the panes already open.

| Key | Effect |
| --- | --- |
| `o` | Reopen panes you closed yourself |
| `q`, Ctrl-C | Leave the controller; panes stay open |

**Choosing a backend.** Without `--backend`, `hivectl panes` uses:
1. zellij or tmux, whichever runs this terminal. When one runs inside the
   other, it picks the one that owns this terminal.
2. Ghostty, when run directly in a Ghostty terminal on macOS.
3. Otherwise it exits with `pane_backend_unavailable`; use `hivectl tui` there.

**zellij.**
- Each root agent gets its own tab, named after the agent's name, or its role and
  short ID. The name is taken when the tab opens; a later rename shows in the view
  and the controller's list.
- Descendants are stacked in their nearest ancestor's tab, up to six per tab;
  further children share a `children` tab.
- One controller runs per zellij session; a second one exits with
  `pane_controller_running`.

**tmux (3.0 or newer).**
- Each root agent gets a window in the controller's session, named like a zellij
  tab. Descendants split that window, tiled up to four agents;
  further children share a `children` window.
- New windows and panes never take focus from where you are.
- Controller-opened views identify their current agent through the pane title;
  `/attach` updates that identity, and exiting clears the title so a surviving
  shell is no longer identified as a view. Before a valid title arrives, the start
  command identifies the agent, so slow startups and manually opened views
  without `--title` are recognized too. Navigating away counts as closing the
  previous agent's view (press `r` in the controller to reopen closed views).
- One controller runs per tmux session; a second one exits with
  `pane_controller_running`.
- Agent windows have `remain-on-exit` off, so views close when their agents end.
  If a hook keeps one open anyway, the controller closes its own dead views,
  never other panes.

**Ghostty (1.3 or newer, macOS).**
- Each root agent gets a tab in the controller's window, named like a zellij tab.
  Descendants split that tab, up to a 2×2 grid; further children share a
  `children` tab.
- New tabs and splits never take keyboard focus from where you are typing.
- The first run asks macOS for Automation permission. If it is denied, allow it
  in System Settings → Privacy & Security → Automation.
- Views find each other by terminal title (`hive:<agent ID>`), so Ghostty's
  `title` option must be unset.
- One Ghostty controller runs per state directory.

| Option | Effect |
| --- | --- |
| `--root AGENT_ID` | Only this agent and its descendants |
| `--max-panes N` | Most panes open at once, 1–32 (default 16); further agents wait for a free pane |
| `--linger SECONDS` | How long an ended agent's pane stays, 0–60 (default 3) |
| `--backend zellij\|tmux\|ghostty` | Skip detection |
| `--no-mouse` | Open views without mouse support |

A pane that fails to open is retried after 5 s, 30 s and 2 min, then listed as
failed in the controller.

## Supported terminals

Hive runs on macOS and Linux.

| Terminal | `hivectl tui`, `hivectl attach` | `hivectl panes` | Question notification |
| --- | --- | --- | --- |
| zellij | Yes | Yes | Not passed on |
| tmux 3.0 or newer | Yes | Yes | Not passed on |
| Ghostty (macOS) | Yes | Yes (1.3 or newer) | Yes |
| Ghostty (Linux) | Yes | Inside zellij or tmux | Yes |
| iTerm2, WezTerm | Yes | Inside zellij or tmux | Yes |
| Terminal.app | Yes | Inside zellij or tmux | No |
| Other terminals | Yes | Inside zellij or tmux | If the terminal shows OSC 9 notifications |

Ghostty on Linux has no scripting interface for tabs and splits yet, so
`hivectl panes` drives Ghostty itself only on macOS.

Clicks and the wheel work in `hivectl tui` and `hivectl attach` in every terminal
above. tmux passes them to its panes only after `set -g mouse on`; zellij does
by default.

## Limits and troubleshooting

- **Shared daemon budget.** Each open view, the `hivectl tui` sidebar, and each
  `hivectl panes` controller and pane holds one of the daemon's 32 waiting
  requests. When they are all taken, views show "waiting for a viewer slot"
  and retry. `--max-views` and `--max-panes` keep room for others.
- **View cap.** A closed `hivectl tui` view's request can stay open for up to 25 s,
  so it still counts toward `--max-views` meanwhile. At the cap, opening another
  agent replaces the least recently used view without a write in flight, or
  asks you to retry shortly.
- **Daemon restarts.** Views show "disconnected" and reconnect without losing
  or repeating transcript entries.
- **Large trees.** The sidebar and `hivectl panes` follow at most 1,000 agents and
  say so when the tree is truncated.
- **Scripts.** Views need a terminal for both input and output; without one
  they exit with `interactive_terminal_required`. Use `hivectl live --follow` or
  `hivectl read --follow` in scripts.

The [CLI reference](cli-reference.md#open-a-chat-view) lists the same commands
with every other Hive command.
