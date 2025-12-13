# AGENTS.md

## Build/Test Commands
- **Install all**: `just install` (runs `uv sync` + cargo installs)
- **Python tests**: `uv run pytest tests/` or single test: `uv run pytest tests/test_mail.py::TestGetFolder::test_get_inbox -v`
- **Rust check/build**: `cargo check` / `cargo build` (from h8-client/)
- **Rust tests**: `cargo test` or single: `cargo test test_name` (from h8-client/)
- **Rust format**: `cargo fmt` then `cargo clippy`

## Code Style
- **Python**: Type hints required, docstrings on modules/functions, imports grouped (stdlib, external, local)
- **Rust**: Use `thiserror` for errors, `anyhow` for CLI. Re-export public types from lib.rs. Doc comments on public items.
- **Naming**: snake_case (Rust/Python), PascalCase for types. No emojis in code/docs/commits.
- **Tests**: Class-based in Python (`class TestFoo`), use mocks for external services. Rust: assert full structs, not fields.

## Architecture
- `h8-service/`: Python FastAPI backend (exchangelib for EWS)
- `h8-client/`: Rust workspace (h8-core lib, h8-cli, h8-tui)
- Config: `$XDG_CONFIG_HOME/h8/config.toml`, data: `$XDG_DATA_HOME/h8/`

## Issue Tracking (bd/beads)
Use `bd` for all tracking: `bd ready --json`, `bd create "title" -t bug -p 1`, `bd close <id>`.
Always commit `.beads/issues.jsonl` with code changes. Store planning docs in `history/`.
