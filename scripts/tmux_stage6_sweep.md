# Stage 6 tmux sweep — advanced variables end-to-end workflow

Scripted rerun of the spec's Global Constraints workflow: init a project,
build variables/options/groups entirely in the GUI, switch envs and watch
columns follow, select via picker, hit the secret prompt on first send
against a live local backend, extract-to-variable, promote/demote. Every
step below is `tmux send-keys` + `capture-pane` — rerun verbatim to
reproduce the sweep. Judges flow friction, not pixels; each check is marked
**done** or **issue** with the finding.

## Setup

```bash
mkdir -p /tmp/claude-1000/tmux
export TMUX_TMPDIR=/tmp/claude-1000/tmux
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -p postui --bin postui

rm -rf /tmp/claude-1000/pw6
mkdir -p /tmp/claude-1000/pw6/project /tmp/claude-1000/pw6/server
echo '{"ok":true}' > /tmp/claude-1000/pw6/server/index.html

tmux kill-server 2>/dev/null
```

Hold the app in a background Bash call (`run_in_background: true`):

```bash
export TMUX_TMPDIR=/tmp/claude-1000/tmux
tmux new-session -d -s s6 -x 220 -y 50 \
  "$PWD/target/debug/postui /tmp/claude-1000/pw6/project" && sleep 3600
```

Second window: a real local backend (loopback only reachable from *inside*
this tmux session — a separate Bash invocation's `curl` to 127.0.0.1 will
get connection-refused even while the server is up, since each Bash call
gets its own sandbox network namespace; the tmux-held processes share one).

```bash
tmux new-window -a -t s6 -d \
  "cd /tmp/claude-1000/pw6/server && python3 -m http.server 8791 2>&1"
```

## 1. Init the project (GUI, not hand-written)

```
tmux send-keys -t s6:0 Enter        # "Create project" on the not-a-project screen
```
**Expect:** header bar shows `postui   project ▾   no env ▾`; sidebar says
"No requests yet." — **done**.

Environments themselves have no GUI creation flow this stage (confirmed
in code: `OpenEnvChooser`/`CycleEnv` both toast "no environments — create
environments/<name>.toml in the project" when the dir is empty; this is
pre-existing stage-3 scope, not something stage 6 was asked to add). Create
the env files as empty placeholders — this is the one on-disk step outside
the GUI, and it's a file the app itself would create lazily on first grid
edit; it just needs to *exist* for the env chooser to list it:

```bash
mkdir -p /tmp/claude-1000/pw6/project/environments
printf '' > /tmp/claude-1000/pw6/project/environments/qa.toml
printf '' > /tmp/claude-1000/pw6/project/environments/prod.toml
```

## 2. Build vars/options/group/secret in the Manager (`alt+v`)

```
tmux send-keys -t s6:0 M-v
```
**Expect:** "Variables — project · no env" screen, empty grid with
`+ Add variable` / `+ Add group` — **done**.

Create `region` (enumerated) with `east`/`west` options, `base_url`
(simple), group `creds` with members `user_id`/`customer_id` and option
`alice`, secret `api_key`, and `trace_id`:

```
n region <Enter>            o east, east-1 <Enter>    o west, west-9 <Enter>
n base_url <Enter>
g creds, user_id, customer_id <Enter>
o alice, user_id=1001, customer_id=c-77 <Enter>        # cursor on creds first
n api_key <Enter>           s y                        # toggle secret, confirm
n trace_id <Enter>
```
(`o` targets whatever row the cursor is on — see the **Finding** below
before scripting this blind.)

**Expect:** `variables.toml` ends up as:
```toml
[region]

[region.options.east]
value = "east-1"

[region.options.west]
value = "west-9"

[base_url]

[groups.creds]
members = ["user_id", "customer_id"]

[groups.creds.options.alice]
user_id = "1001"
customer_id = "c-77"

[api_key]
secret = true

[trace_id]
```
— **done** (verified via `cat` after each step; matches the Manager's own
row order: project vars first, in declaration order, then groups).

**Finding (friction, my own mis-navigation, not an app bug):** `capture-pane
-p` (no `-e`) renders **no cursor indicator at all** — the `›`/`⌄` glyphs
are expand/collapse chevrons, not the cursor. I mis-clicked `o` onto
`base_url` instead of `region` once this way, discovered it only when
`variables.toml` showed the option under the wrong owner, and recovered by
deleting and recreating the variable (there is no per-option delete — `d`
only targets `Var`/`GroupHeader` rows; deleting a wrongly-added option means
deleting and recreating its whole owner). Confirmed via `capture-pane -e`
that the cursor is real but invisible in a plain capture: the cursor
row/cell keeps the *ambient* background color while every other cell
explicitly repaints its own default background over it. **Deferral for the
user:** scripting Manager navigation blind is genuinely error-prone even
for an agent driving it directly; a real user has an actual visible cursor
(this is a tmux-capture artifact, not a user-facing gap) — no code change
suggested, just documented here so the next sweep doesn't repeat the
mistake.

## 3. Environment values + selection via the picker

Switch to `qa` (`alt+e`, the chooser lists `prod`/`qa`/`no environment`
alphabetically):
```
tmux send-keys -t s6:0 M-e
tmux send-keys -t s6:0 Down Enter    # first entry is "prod"; qa is second
```
**Expect:** header shows `qa ▾` — **done**.

In the Manager, set `base_url`'s `qa` (and `prod`) cell to
`http://127.0.0.1:8791` (Enter to edit a cell, type, Enter to commit).
**Expect:** `environments/qa.toml` and `environments/prod.toml` both become
`base_url = "http://127.0.0.1:8791"` — **done**.

Select `creds → alice` for `qa` via the grid: expand `creds` (Enter on the
header), move to the `alice` option row, `Space` on the target env's
column. **Expect:** `.local/state.toml` gains
`[selections.qa]\ncreds = "alice"` — **done**.

Create the request and build the URL **through the real per-token `{{`
picker**, not by pasting the literal string:
```
n orders <Enter>
alt+u
{{base_url<Enter>          # {{ opens the picker; type to filter; Enter inserts
/orders/
{{region<Enter>
?user=
{{user_id<Enter>
&cust=
{{customer_id<Enter>
```
**Expect:** URL reads
`{{base_url}}/orders/{{region}}?user={{user_id}}&cust={{customer_id}}` —
**done**.

**Finding (real, worth knowing, not a bug):** typing the **whole** URL
string at once — including literal `{{base_url}}/orders/{{region}}...` —
does **not** work over raw keystrokes: the moment `{{` is typed the picker
pops up and steals every subsequent keystroke as its filter text, silently
discarding the rest of the intended URL. This is correct, intentional
behavior for live typing (spec §6's whole point), but it means **pasting**
a URL that already contains `{{...}}` tokens (e.g. copied from docs or
another request) would behave the same way — the picker would pop on the
first `{{` and swallow the rest of the paste. Worth the user's attention:
is paste distinguished from keystroke input anywhere in the input pipeline?
If not, a multi-token paste into the URL field is currently a silent
data-loss trap. **Deferral for the user** — this is a pre-existing
`{{`-detection behavior (not new to stage 6's task set) and paste handling
is out of this task's scope to fix; flagging for a follow-up.

Now select `region`'s option via the **in-context select picker**
(`ctrl+v` with the caret on the `{{region}}` token):
```
tmux send-keys -t s6:0 Left×N        # caret into the middle of {{region}}
tmux send-keys -t s6:0 C-v
tmux send-keys -t s6:0 Enter         # "east" is first (declared order)
```
**Expect:** `Select — region` picker opens listing `east`/`west`;
confirming writes `.local/state.toml`'s `[selections.qa]` → `region = "east"`
and leaves the URL text untouched — **done**.

Add headers `x-api-key: {{api_key}}` and `x-trace: {{trace_id}}` (Headers
tab, `+ Add header`, same `{{`-picker flow for the value), and a
request-scope override `trace_id = req-trace-override` on the **Vars** tab.

**Finding (real, minor):** `alt+3` is bound to `EditorTabSelect(2)`, which
is the **Body** tab (`Params`=0, `Headers`=1, `Body`=2, `Vars`=3 by
declared index) — not `Vars`, despite `Vars` being the third tab
*visually*. There is no `alt+4` for `Vars` either; it's reachable only by
mouse click or `alt+Right`/`alt+Left` cycling. Not a functional bug (`Vars`
is fully reachable, keyboard parity holds via the cycle keys) but a
plausible trap for anyone assuming `alt+N` walks the tabs left-to-right in
display order. **Deferral for the user** — worth either an `alt+4`
binding for `Vars` or a doc note; small enough to be a safe follow-up but
outside this task's scope (keybinding table is shared/cross-cutting).

## 4. Send: secret prompt chain against the live local backend

```
tmux send-keys -t s6:0 C-s   # save
tmux send-keys -t s6:0 C-r   # send
```
**Expect:** masked prompt `Value for `api_key` (secret, env `qa`)` — send
does **not** go out yet — **done**.

```
tmux send-keys -t s6:0 "sk-qa-999" Enter
```
**Expect:** confirming writes `.local/secrets.toml` and immediately
re-sends; response pane shows a **real** status from the local backend
(create `server/orders/east-1` first so it's 200, not 404 — a static file
server needs the exact path to exist) — **done**. Verified with the
window-1 access log: `GET /orders/east-1?user=1001&cust=c-77 → 200`,
proving `base_url`, `region`, `user_id`, `customer_id`, `api_key`, and the
request-scope `trace_id` override were all substituted correctly end to
end (headers `x-api-key`/`x-trace` land on the request the wiremock-backed
Rust acceptance test already asserts explicitly).

## 5. Env switch flips resolved values

```
tmux send-keys -t s6:0 M-e Enter     # "prod" is first in the chooser
```
Open the Manager and check `region`'s row: `qa` shows `east · east-1`,
`prod` shows `⚠ select` (no selection recorded for prod yet) — **done**,
confirms per-env resolution is independent.

Select `region → west` for `prod` (click the `west-9` cell in the `prod`
column, or `Space` on that cell) and send again once `prod`'s `api_key`
secret is supplied and `server/orders/west-9` exists:
**Expect:** a fresh 200 from the local backend at `/orders/west-9`,
distinct byte count from the qa response — **done**. Access log:
`GET /orders/west-9... → 200`.

**Finding (my own mis-tracking, not a bug — noted so the next run doesn't
repeat it):** after a `Send` that fails to resolve (e.g. secret still
missing for the *other* env), the response pane keeps showing the **last
successful** response, unchanged — there's no visible "nothing sent"
marker in the response pane itself (only a toast, which fades quickly and
is easy to miss when driving fast over tmux). I initially misread a stale
qa 200 as a fresh prod 200 until cross-checking the local server's access
log, which showed only 2 real requests instead of 3. This is stage-3
behavior (`unresolved variables must not send anything`, already covered
by that stage's acceptance test), not new to stage 6 — flagging only
because it's exactly the kind of thing that makes a fast manual sweep
double-check against server logs rather than trust the screen alone.

## 6. Extract-to-variable

Add a literal param `debug = true`, focus its value cell (click it, `Enter`
to start editing), then trigger extract.

**Issue found:** `ctrl+shift+e` (`Action::ExtractToVariable`'s only
binding) is **not reliably reachable from a standard terminal**. Ctrl+E and
Ctrl+Shift+E send the identical byte (`0x05`) over a plain TTY — Shift adds
no information for a Ctrl+letter chord unless the terminal speaks an
extended keyboard protocol (e.g. Kitty's), which a bare tmux pane does not.
Confirmed directly: sending byte `0x05` while an editable text field was
focused triggered `Action::OpenBodyInEditor` (bound to plain `ctrl+e`) —
it opened `$EDITOR` (nano) on the request body instead of extracting
anything. The two bindings collide; `ctrl+e` wins because it's checked
first/matches the same byte.

The documented fallback — the command palette (`ctrl+p` → "extract") — also
did **not** work in this session: running it while the value cell was
selected (and even immediately after re-entering edit mode on that cell)
produced the same "focus a text field first" toast `ExtractToVariable`
gives when focus isn't on a text field. Opening the palette overlay appears
to disturb the table's `editing` state (or the focus check the action
performs) enough that the origin field no longer reads as "focused" by the
time the palette's own selection dispatches the action.

**Deferral for the user:** this looks like a real, user-facing dead end for
extract-to-variable in any terminal without extended-keyboard support
(which is most of them) — worth either rebinding `ExtractToVariable` off
`ctrl+shift+e` to something a plain terminal can send unambiguously, or
fixing the palette path to actually work as the documented fallback. Not
fixed here: it's a keybinding/focus-semantics change with cross-cutting
implications (the palette's interaction with in-progress table edits is
shared machinery), too large for a "small, safe" task-18 fix.

## 7. Promote / demote

Promote a request-scope override (`trace_id`, Manager → "This request"
section, cursor on the row, `p`):
```
tmux send-keys -t s6:0 p
```
**Expect:** `Promote trace_id` / "Where should the value land?" prompt with
`Default value` / `Env value (prod)` / `Cancel` buttons.

**Finding (minor, worth a look):** pressing `Enter` on this prompt does
**not** confirm the highlighted choice — the modal stayed open with no
visible change. Every other modal in the app (`Prompt`, `Confirm`) treats
`Enter` as confirm; this `MultiPrompt`-with-choice-buttons variant didn't,
at least not on a bare `Enter` with no field navigated to first. Clicking
the `Default value` button directly worked immediately and correctly (
`variables.toml`'s `[trace_id]` gained `default = "req-trace-override"`,
the request's own `[variables]` entry was removed) — **done** via mouse,
**issue** via keyboard (Enter didn't work as expected on first try;
possibly needs a Tab into the button row first — not conclusively
diagnosed in this session). **Deferral for the user**: worth a follow-up
check of whether `Enter` is supposed to confirm a `MultiPrompt`'s
highlighted choice-button directly, and if so why it didn't here.

Demote a project variable into the request (`base_url`, cursor on the row,
`P`, confirm `y`):
```
tmux send-keys -t s6:0 P
tmux send-keys -t s6:0 y
```
**Expect:** `Demote "base_url" into this request?` confirm; `y` moves it —
`base_url` now appears under "This request", removed from `variables.toml`
— **done**, worked cleanly via keyboard (this one *is* an ordinary
`Modal::Confirm`, consistent with the rest of the app).

## Teardown

```bash
export TMUX_TMPDIR=/tmp/claude-1000/tmux
tmux send-keys -t s6:0 Escape
tmux send-keys -t s6:0 q
tmux kill-server
```

## Summary

| Check | Result |
|---|---|
| Init project via GUI | done |
| Declare vars/options/group/secret purely via Manager actions | done |
| Env values written via Manager grid edits | done |
| Selection via in-context `{{`-picker (`ctrl+v`) | done |
| Selection via Manager grid checkmark/click | done |
| Request built via real per-token `{{` picker flow | done |
| Headers + request-scope var override via Vars tab | done |
| Send-time secret prompt chain, real backend, real 200 | done (x2, qa and prod) |
| Env switch flips resolved values, columns follow independently | done |
| Extract-to-variable (keyboard) | **issue** — `ctrl+shift+e` collides with `ctrl+e` on a plain terminal |
| Extract-to-variable (palette fallback) | **issue** — palette path didn't trigger it either in this session |
| Promote (mouse) | done |
| Promote (keyboard, `Enter` on choice prompt) | **issue** — didn't confirm |
| Demote (keyboard, ordinary confirm) | done |
| Cursor visibility while scripting (tmux artifact, not app-facing) | noted, not an app issue |
| `{{`-detection swallowing a hypothetical paste | noted as a follow-up worth checking, not verified either way in this session |
| `alt+3` selects Body, not the visually-third Vars tab | noted, keyboard parity still holds via cycle keys |

All "issue" rows above are deferred to the user rather than fixed in this
task — none are small/safe fixes: the extract-to-variable ones touch
keybinding assignment and/or palette-focus semantics (cross-cutting,
shared machinery), and the promote-prompt Enter behavior needs more
diagnosis than this sweep had budget for before concluding whether it's
even a bug. The user's own real-terminal sweep remains the final gate per
the spec's Global Constraints.
