# Capabilities (least privilege)

JSON cannot hold comments, so the reasoning lives here.

A capability grants a set of windows a set of permissions. The webview can
only call Rust and Tauri APIs that some capability allows.

- `default.json` gives the `main` window `core:default`: the small set of
  safe core APIs (events, window/app metadata, resources, menu/tray basics).
  It grants **no** filesystem, shell, HTTP or dialog access.
- Our own `#[tauri::command]`s (like `ping`) are allowed by default for all
  windows of this app. Tauri only restricts them if we later declare an
  app manifest in `build.rs`.
- Anything powerful (reading repos, running `git`) will happen in the Rust
  core behind our own commands, not by handing the webview fs/shell plugins.
  Add permissions here only when a task needs them, one by one.
