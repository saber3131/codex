# 造物主 Creator — modifications to OpenAI Codex

This branch (`creator`) is OpenAI Codex (Apache-2.0) with the following changes. All
Creator logic is kept in two new files so upstream releases merge cleanly; the other
files only gain small, commented hooks.

New files

- `codex-rs/cli/src/creator.rs` — Creator startup (data in `~/.creator` / `CREATOR_HOME`,
  default config, model catalog), 紫微星通行证 login/logout/status, `creator sync`, and the
  B31.Si-1.3 commander/expert plan built from 龍華樹 leaderboard ranks.
- `codex-rs/core/src/creator_alias.rs` — maps catalog-only model slugs (e.g. `b31-si-1.3`)
  to the model actually served, from the `CREATOR_MODEL_ALIASES` environment variable.
  Identity function when the variable is unset.
- `.github/workflows/creator-release.yml` — merges the newest upstream Codex and builds
  Windows / macOS / Linux releases on GitHub-hosted runners.
- `creator/` — this document and the Chinese user guide.

Modified files

- `codex-rs/cli/Cargo.toml` — binary renamed `codex` → `creator`; `reqwest` dependency.
- `codex-rs/cli/src/main.rs` — calls Creator startup first; `login`/`logout`/`status`/`sync`
  run the Creator commands; upstream OpenAI login is kept as hidden `openai-login` /
  `openai-logout`; CLI name/usage text.
- `codex-rs/core/src/lib.rs`, `codex-rs/core/src/client.rs` — request model slug passes
  through `creator_alias::api_model_slug`.
- `codex-rs/tui/src/history_cell/session.rs`, `codex-rs/exec/src/event_processor_with_human_output.rs`
  — header title "造物主 Creator".

Defaults written to the generated `config.toml` (user-editable): provider 龍華樹
(`https://b31.si/lh/v1`, Responses API), OpenAI analytics, feedback upload and update checks
off, hosted web search off. The model catalog keeps only the models the 龍華樹 Responses
endpoint serves (it is probed with an unknown model name, which is not billed), and uses the
freeform `apply_patch` tool, which 龍華樹 converts for every upstream provider.

Startup also keeps `config.toml` current through numbered migrations (revision 1 turns
off Codex's background-server auto start, which downloads an official Codex build), writes a
managed global `AGENTS.md` listing other coding CLIs found on PATH, and — when Windows
Explorer starts `creator.exe` by double-click — signs in interactively, works in
`Documents\造物主工作区`, and keeps the window open if it stops with an error.

Creator is not an OpenAI product and does not use OpenAI trademarks for its own branding.
