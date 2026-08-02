# Built-in rung-0 adapters

These ship with artist and are the *lowest*-priority layer: anything in
`$ARTIST_CONFIG_DIR/computer/adapters/` with the same filename replaces the one
here entirely. That ordering is deliberate — a built-in should never override
something a user wrote for their own machine.

## Why these three

They are the three shapes rung 0 takes, so between them they document the format
by example rather than by prose:

- **`mpris.toml`** — a **D-Bus interface that already exists**. Nothing is being
  substituted; the application genuinely offers this API and driving its window
  instead would be strictly worse.
- **`git.toml`** — a **CLI standing in for a GUI**. The graphical client and the
  command line operate on the same repository, so the cheaper one wins and the
  GUI updates itself.
- **`files.toml`** — a **desktop service standing in for a window**. `xdg-open`
  performs the same lookup the double-click does.

## Writing your own

Drop a `.toml` in `$ARTIST_CONFIG_DIR/computer/adapters/`. It is picked up
without a restart. The three fields:

```toml
name = "myapp"
match_app_id = ["*myapp*"]        # globbed against app id or argv[0]

[[action]]
name = "pause"
description = "Shown to the model, so write it for a reader."
dbus = { service = "…", path = "…", interface = "…", method = "…" }
# or: cli  = { argv = ["mycli", "pause", "{value}"] }
# or: http = { method = "POST", url = "http://127.0.0.1:8080/pause" }
```

`{value}` is replaced by the text of the step invoking the action.

Two limits worth knowing, both deliberate:

- **`http` reaches loopback only.** An adapter template is trusted but the value
  spliced into it comes from the model, so `http://127.0.0.1:8080/open?to={value}`
  would otherwise be one careless adapter away from an outbound-request
  primitive. Anything genuinely remote belongs behind a CLI you chose to install.
- **No shell.** `argv` is executed directly, so a `{value}` containing `;` or
  backticks is an argument and not a command.
