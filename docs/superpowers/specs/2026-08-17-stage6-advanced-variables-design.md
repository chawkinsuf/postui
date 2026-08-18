# Stage 6 — Advanced Variables

*2026-08-17 · Status: user-approved section-by-section in chat. Implements master
spec §4 ("Variable System") in full: enumerated options, groups, secrets,
request scope, the Variable Manager screen, and the zero-friction in-context
flows. Builds on the stage-3 variable plumbing (`variables.toml`, env files,
`{{var}}` substitution, picker).*

## 0. Goals & scope

Variables are the product differentiator (master spec pillar 2). After this
stage:

- Variables can be **simple**, **enumerated** (keyed options with
  descriptions), or **grouped** (options that set several variables
  atomically), with values varying per environment in every form.
- **Secrets** live only in `.local/secrets.toml`, are prompted for at send
  time, and are masked everywhere by default.
- Requests can carry **request-scoped** overrides.
- A full-screen **Variable Manager** edits all of it; the picker gains
  option selection, group previews, inline option creation/editing, and
  extract-to-variable.
- **Everything is editable in the GUI. Hand-editing TOML is never required**
  — this is an acceptance criterion, not an aspiration.

Out of scope: script-set variables (next stage; a precedence slot is
reserved), body-text extract-to-variable (needs body selection, deferred),
history/console redaction (console stage).

## Global constraints

- **tmux-driven usability verification (user requirement):** every UI task is
  verified live over tmux before it is called done (recipe in the
  tmux-tui-driving memory / stage-4 plan), and the stage ends with a scripted
  end-to-end workflow sweep — init a project, build variables/options/groups
  entirely in the GUI, switch envs and watch columns follow, select via
  picker, hit the secret prompt on first send, extract-to-variable,
  promote/demote — judging flow friction, not just pixels. The user's manual
  sweep in a real terminal is the final gate.
- Painted-UI conventions (stage 5 + post-stage-5 rounds) apply to every new
  surface: painted buttons/fields/chips, hover, hit-map dispatch, keyboard
  parity for every mouse action and vice versa.
- All user-facing failures are toasts/friendly errors, never crashes; all
  file writes atomic.

## 1. On-disk model

### 1.1 `variables.toml`

One top-level table per variable (as stage 3); options are **keyed**
sub-tables — the key is the stable identity that per-env overrides address
(description text would be fragile) and is shown in the picker:

```toml
[base_url]
description = "API root"
default = "http://localhost:8080"

[api_key]
description = "service API key"
secret = true                      # value lives in .local/secrets.toml

[user]
description = "seeded test user"
[user.options.alice]               # keyed, order-preserving (indexmap)
description = "admin, active sub"
value = "1001"
[user.options.bob]
description = "expired trial"
value = "2002"

[groups.test-user]                 # `groups` is a reserved top-level name
description = "user with linked customer"
members = ["user_id", "customer_id"]
[groups.test-user.options.alice]
description = "admin, active sub"
user_id = "1001"
customer_id = "c-77"
```

Rules (violations = friendly parse errors naming the fix):

- `options` and `groups` are **reserved names**: no variable or group may be
  named either.
- Group names share one namespace with variable names — no collisions.
- Group members are implicitly declared by the group. A member *may* also
  have its own top-level table to carry a description, but a member with its
  own `options`, a `secret = true` member, or a variable in two groups is an
  error.
- Secrets are simple: `secret = true` with `options` or a `default` is an
  error (a default would commit a secret value).
- Option keys satisfy the variable-name rules (ASCII alnum/`_`/`-`,
  non-empty, case-sensitive).

### 1.2 `environments/<env>.toml`

Flat pairs for simple values (unchanged), plus one reserved `options`
section:

```toml
base_url = "https://qa.example.com"

[options.user.alice]               # override a shared option's value here
value = "9001"
[options.user.qa-only]             # new key = env-specific extra option
description = "exists only in qa"
value = "3003"
[options.test-user.alice]          # group form: member values directly
user_id = "9001"
```

**One merge rule covers every case:** env option tables merge by key onto
the variable's (or group's) shared list — overriding `value`/member values
and `description` where given, adding options for new keys. A variable whose
declaration has *no* shared options gets the env's list wholesale (the
per-env-list form). `[options.<name>]` where `<name>` is not a declared
variable or group at all, or is a secret, is an error.

A variable is *enumerated in an env* when it has any options after this
merge; the same variable may be enumerated in one env (env-defined list) and
simple in another. Strictness where ambiguity would lurk:

- A flat env value for a variable that is enumerated in that same env is an
  error (which layer wins would be guesswork).
- A flat env value for a secret variable is an error (it would commit a
  secret).
- Undeclared flat env values remain usable in resolution (stage-3 leniency
  preserved); only declared variables appear in pickers.

### 1.3 Local state

```toml
# .local/state.toml (additions)      # .local/secrets.toml (new)
[selections.qa]                      [qa]
user = "alice"                       api_key = "sk-qa-..."
test-user = "alice"                  [prod]
                                     api_key = "sk-prod-..."
```

- Selections are per environment: `name → option key` (variables and groups
  share the map; the namespace is shared anyway). A selection naming a
  missing option key degrades to unselected (with a toast on load), never an
  error.
- **No auto-selection:** an enumerated/group variable with no selection is
  unresolved — send blocks naming it (deliberate: a silently wrong test user
  is worse than one picker trip; the picker is one keypress away).
- `secrets.toml` is sectioned per env, flat pairs inside. Both files are
  created lazily on first write; both live under the already-gitignored
  `.local/`.

### 1.4 Request files

A request file gains a `[variables]` table using the same entry form as
params/headers — `name = "value"` or `{ value, enabled }` — so an override
can be toggled off without deleting it. Disabled entries do not participate
in resolution. Values are literal simple strings: no options, no groups, no
`{{}}` re-expansion (single-pass, consistent with the resolver).

## 2. Core resolver & precedence

New model in `postui-core` (extending `project.rs`):

- `VarDecl` gains `secret: bool` and `options: IndexMap<String, OptionDecl>`
  (`{description, value}`); `Variables` gains
  `groups: IndexMap<String, GroupDecl>` (`{description, members, options}`,
  a group option holding `{description, values: member → value}`).
- `EnvData` replaces the flat env map: `{values, options}` where `options`
  mirrors the keyed override tables.
- `Selections` (per-env `name → option key`) and `Secrets` (per-env map) are
  separate inputs loaded from `.local/`.

The heart of the stage is one pure function:

```
resolve_env(&Variables, &EnvData, &Selections, &Secrets) → Resolved
```

`Resolved` holds the flat `values: IndexMap<String, String>` plus per-name
metadata: `Simple | Enumerated{selected} | GroupMember{group} | Secret |
NeedsSelection | MissingSecret`. Names needing a selection or secret are
**omitted** from `values` — `prepare()`'s existing missing-set machinery
flags them — but the metadata lets the UI say "needs a selection" or run the
secret prompt instead of a generic "undefined variable". The picker and the
Variable Manager read the same `Resolved`: one source of truth.

**Precedence per name, first hit wins:**

1. Request `[variables]` (enabled entries; applied in `prepare()` as an
   overlay on the flat map)
2. *(reserved: script-set values, scripting stage)*
3. Secret value for the active env
4. Enumerated/group: the selected option's value from the env-merged list
5. Simple env value
6. Declaration `default`

Group members resolve from the group's selected option; `prepare()`'s
unresolved-send block distinguishes the three causes in its message
(undefined / needs selection / missing secret) and the latter triggers the
secret prompt flow instead of blocking.

## 3. Secrets behavior

- **Send-time prompt:** when `prepare()` reports `MissingSecret` names, the
  send pauses and a masked-input modal prompts for each in turn — "Value for
  `api_key` (secret, env `qa`)" — writing each to `secrets.toml` under the
  active env, then the send proceeds automatically. Esc cancels the whole
  send. This is the entire first-run story after cloning a shared project.
- **Masked everywhere by default:** Manager and pickers render secret values
  as `●●●●`; the Manager has a reveal toggle on the focused cell, and secret
  editing uses masked input with the same toggle. A screen-share never leaks
  a token by default.
- **Names, never values, in incidental output:** toasts, unresolved
  messages, and parse errors print secret names only. (Substituted request
  content carries the real value — that is its job.)
- **Flag transitions in the GUI:** secret → non-secret confirms and moves
  nothing automatically (the local value is offered for copy, not silently
  promoted into git-tracked TOML); non-secret → secret offers to move env
  values into `secrets.toml` and strip them from env files.

## 4. Request scope UX

- **Vars tab:** the request editor gains a Vars tab (after Headers) reusing
  the existing table editor wholesale — selection/expansion, enable toggle,
  delete-confirm, ghost + Add row — with its count in the tab label. This is
  the day-to-day edit surface for request scope.
- **Shadowing is visible:** a row shadowing a project variable shows the
  shadowed value inline (dim "overrides qa: 1001").
- **Promote / demote (Manager):** promote moves a request entry into
  `variables.toml`, a small modal asking where the value lands (declaration
  `default` or the active environment). Demote writes the currently resolved
  value into the open request's `[variables]` and deletes the project
  declaration — after a confirmation reporting usage from a scan of all
  request files for `{{name}}` ("referenced by 4 other requests").
  Enumerated/grouped variables cannot be demoted (request scope is
  simple-only); the Manager says why.

## 5. Variable Manager screen

- **Entry/exit:** `App` gains `Screen { Main, VarManager }` — draw and key
  routing branch on it once; the modal stack works on top unchanged. Opened
  via palette ("Variable Manager") and default keybinding `alt+v`
  (rebindable; `ctrl+v` remains the picker). Esc (no modal/edit open)
  returns to `Main` exactly as left. This enum is also where future screens
  (history, console) will slot in.
- **Layout:** a full-screen grid. Fixed left columns: name, description;
  then one column per environment side by side, horizontally scrollable when
  they overflow. Row order: request-scope section (open request, when any),
  then project variables; groups render as header rows with members
  indented; an enumerated variable/group expands (Enter/click, like the
  sidebar tree) into option sub-rows — key, description, per-env value
  cells. Cells show resolution truth: env value (dim when falling back to
  default), selected option's value with its key, `●●●●` for secrets,
  visible markers for needs-selection / missing-secret.
- **Editing:** arrow/Tab cell navigation; mouse click selects, Enter or
  click-selected-again edits in place with the existing inline line-input
  (masked for secrets). Every mutation in §§1–4 has a keyboard action and a
  painted button: new variable, new group, new option, rename, delete
  (confirm + usage scan), toggle secret, add/remove group member,
  promote/demote, and setting the selected option per env (✓ in the env
  column).
- **Persistence:** each committed edit writes atomically and immediately to
  whichever file owns it (`variables.toml`, env file, `secrets.toml`,
  `state.toml`, request file) — no separate save step; failures toast and
  leave the cell in edit.

## 6. Picker & in-context flows

Two contexts, one component:

- **On an existing `{{var}}` token** (cursor on it, or from the
  needs-selection send block): the dropdown lists the variable's options for
  the active env — key, description, value — current selection marked. For a
  group member it lists the *group's* options, each row previewing every
  member's new value ("alice — user_id 1001 · customer_id c-77"). Enter
  writes the selection to `state.toml` for the active env and toasts; the
  token text never changes. Typing filters.
- **When inserting** (`{{` typed, or `ctrl+v` in a field): autocomplete over
  all defined names — scope-badged (request / project / group member /
  secret), with descriptions; Enter inserts `{{name}}`. The list ends with
  **"new variable…"**, opening the create modal pre-filled with the typed
  name, then inserting the reference.

In-context editing — small modals over the editor, focus returning exactly
where it was:

- **"Add new option…"** — last row of the option picker; key + value +
  description inline; saved to the active environment's option table (the §1
  merge rule makes it an env-specific addition), selected immediately.
- **Edit option in place** — `e` on a highlighted option edits value /
  description without opening the Manager, writing to wherever the option
  lives (shared or env).
- **Extract to variable** — with a literal value in a line-input field or
  table cell, one action (palette + keybinding) prompts for name and
  destination (project default / active env value / this request's
  `[variables]`), writes it, and replaces the field text with `{{name}}`.
  Body text is excluded this stage (no body text selection yet).

## 7. Compatibility, write fidelity, testing

- **Compatibility:** all changes additive; stage-3 projects parse unchanged;
  `secrets.toml` and `[selections]` appear lazily. The one break: `options`
  and `groups` become reserved names — a pre-existing variable so named gets
  a friendly parse error naming the fix. No migration step.
- **Write fidelity:** the Manager writes files users hand-edit and
  git-track, so all edits to shareable TOML (`variables.toml`, env files,
  request `[variables]`) are **surgical `toml_edit` mutations of the loaded
  document** — comments, ordering, and untouched entries preserved; only the
  edited key changes. Extends the pattern `model.rs` already mandates for
  request saves. `.local/` files may serialize fresh, as `state.toml` does
  today.
- **Testing tiers:**
  1. **Core unit tests** — parse shapes and every friendly error in §§1–4;
     the env merge rule; `resolve_env` precedence and metadata;
     promote/demote incl. usage scan; toml_edit round-trips asserting
     comments and unrelated keys survive each Manager mutation.
  2. **TUI `TestBackend` tests** — Manager grid (groups indented, env
     columns, masked secrets, markers), cell navigation/edit, both picker
     contexts, group preview rows, secret prompt chain, Vars tab.
  3. **tmux sweeps** per the Global Constraints, ending in the scripted
     end-to-end workflow acceptance run; the user's real-terminal sweep is
     the final gate.
