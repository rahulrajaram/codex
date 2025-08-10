OpenAI Codex CLI — TUI Markdown + Streaming Context

Summary of changes (August 2025)

- Rich Markdown for assistant messages:
  - File: `tui/src/chatwidget.rs`
  - Final assistant answers are rendered via `crate::markdown::append_markdown` (uses `tui-markdown`) instead of plain text. This preserves code blocks, lists, emphasis, and links.
  - Final reasoning (“thinking”) output is also rendered via markdown.
  - We deliberately do NOT commit partial rows to history for both Answer and Reasoning streams. The user still sees progress via the live overlay; history receives a single, well‑formatted markdown block at finalize.

- Headers and live overlay:
  - “thinking” header (magenta/italic) and “codex” header (magenta/bold) are now emitted at finalize together with the formatted block, keeping header + content contiguous in history.
  - Live overlay still updates during streaming (plain text only) to keep performance smooth.

- Removed 80‑column pin for streaming rows:
  - Previously `RowBuilder::new(80)` caused hard wrapping during streaming.
  - Replaced with `RowBuilder::new(usize::MAX)` wherever the live builder is created/reset so wrapping is deferred to renderers (history cells use `Paragraph::wrap`).
  - This avoids narrow hard‑wraps entering scrollback and makes final markdown blocks more readable.

- CLI-configurable rendering (new):
  - Flags added in `tui` binary:
    - `--max-cols <N>`: cap preview/truncation width (default 80).
    - `--no-max-cols`: disable column cap and enable soft‑wrap for the live overlay.
    - `--live-rows <N>`: number of rows in the transient live overlay (default 3).
  - MCP tool result preview width now respects these settings; the hardcoded `80` is removed.
  - Truncation smarter at word boundaries to reduce mid‑word cuts in previews.

Touched files

- `tui/src/chatwidget.rs`
  - Added `use crate::markdown;`
  - Streaming behavior:
    - `stream_push_and_maybe_commit`: does not commit overflow rows for either stream; only updates the live overlay ring.
    - `finalize_stream`: emits the header, renders full `answer_buffer` or `reasoning_buffer` via markdown, inserts a single block + trailing blank line, then clears buffers and live overlay.
  - Width: initialize/reset `live_builder` with `usize::MAX` instead of `80`.
  - Preview width: passes CLI‑driven `preview_max_cols` into MCP result preview instead of a magic `80`.

- `tui/src/markdown.rs` (unchanged)
  - Provides `append_markdown` which also rewrites file citations like `【F:path†L1-L5】` into clickable URIs (e.g., `vscode://file...`) when a file opener is configured.

- `tui/src/history_cell.rs` (unchanged in this pass)
  - History rendering already uses `Paragraph::wrap(Wrap { trim: false })`, so history blocks wrap correctly to terminal width.

- `tui/src/cli.rs` (new flags)
  - Adds `--max-cols`, `--no-max-cols`, and `--live-rows` to control preview width and overlay wrapping/height at runtime.

- `tui/src/bottom_pane/live_ring_widget.rs`
  - Adds optional soft‑wrap for the live overlay (`Paragraph::wrap(Wrap { trim: false })`) gated by CLI `--no-max-cols`.

- `tui/src/bottom_pane/mod.rs`
  - Plumbs a `live_ring_wrap` option through `BottomPaneParams` to the overlay widget.

- `tui/src/text_formatting.rs`
  - Improves truncation to prefer word boundaries when adding ellipses.

Why this design

- Streaming + markdown is hard to do incrementally without flicker or heavy reflow.
- Keeping a lightweight plain live overlay while rendering a single, rich markdown block at finalize balances readability and responsiveness. Emitting headers at finalize keeps history tidy.
- Avoiding hard‑wrap at 80 ensures the final message is not riddled with narrow line breaks.

Status / Tests

- Local runs: please run `just fmt`, `just fix`, and `cargo test --all-features` before PRs. In constrained sandboxes only `cargo fmt`/`cargo check` may be possible.
- Formatting: `cargo fmt` run.
- Note: the repo’s justfile exposes `just fmt` and `just fix` if you prefer; install `just` locally or run `cargo fmt`/`cargo clippy` directly.

Remaining opportunities / next steps

- Syntax highlighting for fenced code blocks:
  - Approach: detect code blocks and language (pre‑pass with `pulldown-cmark`), run `syntect` highlighting, map tokens to `ratatui::Style`, and emit spans in place of plain code. Apply only on finalize to keep streaming cheap. Make theme configurable.

- Live overlay wrapping:
  - Now configurable via CLI. By default we keep the overlay unwrapped; passing `--no-max-cols` enables soft wrapping.

- Make live preview row count configurable:
  - Implemented via `--live-rows <N>`.

- Streaming markdown (optional):
  - If you want fully formatted “thinking” while streaming, we’d need either a streaming renderer or to re‑emit/replace the last N lines in history each tick. That’s a larger refactor; current approach defers formatting to finalize.

- Exec output TODO:
  - `EventMsg::ExecCommandOutputDelta` is still TODO. We currently only show begin/end plus summary. Wire deltas into an active history cell (with dim styling) to mirror MCP tool output.

- Images in MCP outputs:
  - There is a TODO to show images even when they’re not the first result. Requires refactoring `CompletedMcpToolCall` to handle mixed content.

Operational notes

- Don’t modify sandbox env var logic (`CODEX_SANDBOX_*`) per project convention.
- Before PRs:
  - Run `just fmt` and `just fix` (or `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`).
  - Run tests: `cargo test --all-features` at the workspace root.

Quick checklist when resuming work

- Verify your terminal supports hyperlinks and 24‑bit color if you want the best experience.
- Test a few markdown responses (lists, code fences, long lines, links) to confirm rich formatting in history.
- If desired, enable soft‑wrap for the live ring with `--no-max-cols`.
- Decide on syntax highlighting scope and theme, then implement in `markdown.rs`.
