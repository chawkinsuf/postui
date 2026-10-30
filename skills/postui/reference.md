# postui project files

A project is a folder holding these files. Every example below is one
project, and `postui check` passes on it.

```
project.toml                      name, spaces and environments in order, default headers
variables.toml                    every variable and selector, declared once
environments/<env>.toml           one environment's values and selector options
requests/<space>/<path>.toml      one request per file
.postui/                          the user's own state and secrets: never touch it
```

**Names.** Space and environment names, and every segment of a request's
path, are `a-z 0-9 - _` (lowercase). A request's slug is its path under
`requests/` without `.toml`: `requests/main/orders/list-orders.toml` is
`main/orders/list-orders`, in space `main`, folder `orders`. Display names
(`name = "..."`) are free text. A file or folder whose name starts with `.`
is ignored; any other invalid name is an error in `check`.

Run `postui check` after every change.

## project.toml

```toml
name = "Orders API"
spaces = ["main", "admin"]
environments = ["local", "staging"]

[default_headers]
X-Client = "postui"

[space.admin]
name = "Admin"

[environment.local]
name = "Local"
tls = "insecure"

[environment.staging]
name = "Staging"
```

- `name`: the project's display name (default: the folder's name).
- `spaces`, `environments`: the order they show in. A space is a folder
  under `requests/`, an environment a file under `environments/`; one that
  isn't listed still counts and comes after the listed ones. A new space or
  environment goes in its list too.
- `[default_headers]`: sent with every request, unless the request sets the
  same header. The value can use `{{variables}}`.
- `[space.<name>]`: `name` (display name), and `order`, the sidebar order of
  the space's requests as slugs relative to the space (`["login",
  "orders/list-orders"]`). postui writes `order` when the user reorders;
  leave it alone unless asked. A request it doesn't name still shows.
- `[environment.<name>]`: `name`, and `tls = "verify"` or `"insecure"` to
  force certificate checks on or off for every request in that environment.

## variables.toml

```toml
[base_url]
description = "Where the API is served"

[api_version]
default = "v1"

[token]
description = "Bearer token for the API"
secret = true

[selectors.customer]
description = "The customer the requests act as"
fields = ["customer_id", "customer_email"]

[selectors.locale]
fields = ["lang"]
shared = true

[options.locale.en]
lang = "en-US"

[options.locale.fr]
description = "French"
lang = "fr-FR"
```

- A **variable** is a table `[<name>]` with `description`, `default` (used
  when the environment sets none) and `secret` (`true`: its value lives in
  `.postui/secrets.toml`, never in a project file; a secret has no
  `default`). Every `{{name}}` a request uses must be declared here, as a
  variable or a selector's field.
- A **selector** is `[selectors.<name>]` with `fields`, the variables it
  sets together, and `description`. Each of its options gives every field a
  value; the user picks one option per environment (`--select name=option`
  for a run). A field is declared only by its selector, never as a variable
  too, and can't be secret.
- A **shared** selector (`shared = true`) has the same options in every
  environment, written here as `[options.<selector>.<option>]`. Any other
  selector's options live in each environment's file.
- An option is a table of its fields' values plus an optional
  `description` (so no option can be named `description`).
- Variable and selector names are ASCII letters, digits, `_` and `-`, and
  not `options`, `selectors`, `groups` or `entries`.

## environments/\<env\>.toml

```toml
# environments/local.toml
base_url = "http://localhost:8080"

[options.customer.alice]
description = "A customer with orders"
customer_id = "42"
customer_email = "alice@example.com"

[options.customer.bob]
customer_id = "7"
customer_email = "bob@example.com"
```

```toml
# environments/staging.toml
base_url = "https://staging.example.com"
api_version = "v2"

[options.customer.alice]
customer_id = "1001"
customer_email = "alice@staging.example.com"
```

- Top-level keys are values for declared variables (strings).
- `[options.<selector>.<option>]` are a non-shared selector's options in
  this environment: each sets every one of the selector's fields.
- A new environment needs its file and an entry in `project.toml`'s
  `environments`; its display name goes in `[environment.<name>]`.

## requests/\<space\>/\<path\>.toml

```toml
# requests/main/orders/list-orders.toml
name = "List orders"
method = "GET"
url = "{{base_url}}/{{api_version}}/customers/{{customer_id}}/orders"

[params]
limit = "20"
status = { value = "open", enabled = false }

[headers]
Authorization = "Bearer {{token}}"
Accept-Language = "{{lang}}"
```

```toml
# requests/main/orders/create-order.toml
name = "Create order"
method = "POST"
url = "{{base_url}}/{{api_version}}/orders"
substitute_body = true

[headers]
Authorization = "Bearer {{token}}"
Content-Type = "application/json"

[variables]
sku = "SKU-123"

[body]
type = "json"
text = """
{
  "customer": "{{customer_id}}",
  "items": [{ "sku": "{{sku}}", "quantity": 2 }]
}
"""
```

```toml
# requests/admin/health.toml
name = "Health"
method = "GET"
url = "{{base_url}}/health"
jq = ".status"
```

- `name`: the display name. `method`: `GET` (the default), `POST`, `PUT`,
  `PATCH`, `DELETE`, `HEAD` or `OPTIONS`. `url`: required; it may carry its
  own query string as well as `[params]`.
- `[params]` (query parameters) and `[headers]`: `key = "value"` in order.
  `{ value = "...", enabled = false }` keeps a row but leaves it out of the
  send.
- `{{name}}` in the URL, params and headers is filled in at send. In the
  body it is sent as written unless `substitute_body = true`.
- `[variables]`: values for this request alone. They win over every other
  source, and a name used only here needs no declaration.
- `[body]`: `type = "json"` (the one type) and `text`, the body as written.
  `GET` and `HEAD` keep a body but don't send it.
- `insecure = true` skips certificate checks for this request (an
  environment's `tls` overrides it).
- `jq`: a filter the response pane applies to the body (view only; nothing
  sent changes). `[[jq_view]]` tables (`name`, `filter`) are saved filters,
  `jq_current` names the one in use, `jq_enabled = false` switches the
  filter off. postui writes these when the user filters in the TUI; you
  may add them too, and `run --view` uses them.
- postui adds `Accept: application/json` unless the request sets `Accept`,
  and the HTTP client adds `Host`, `Content-Length` and the like.
- Any other key is an error: `check` names the file.

## Where a value comes from

For each `{{name}}`:

- a `--var name=value` on `run` or `suite` wins over everything, for that
  run only;
- the request's own `[variables]` win, when a row is enabled;
- otherwise a selector's field takes the selected option's value (from the
  environment's file, or `variables.toml` for a shared selector);
- a variable takes the environment file's value, else its `default`;
- a secret takes its value from `.postui/secrets.toml`, which the user fills
  in.

A value is used as written: a `{{name}}` inside a value is not filled in.
`base_url = "https://{{host}}"` would send `https://{{host}}`, so postui
refuses to send a request whose URL or query parameters use such a value
(`check` reports it as an error), and `check` warns about every value that
holds a `{{name}}`, on the file it is written in. Give each environment the
whole value instead (`base_url = "https://staging.example.com"`).

`postui vars` shows each variable's source; `postui run <slug> --dry-run`
shows the request filled in.
