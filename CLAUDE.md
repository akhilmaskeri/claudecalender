# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A Rust CLI tool that displays Claude Code usage for the current subscription period as an interactive terminal UI. It reads the plan from local credentials, fetches the billing cycle from the claude.ai API using a browser session cookie, scans local session JSONL files, and presents a neofetch-style TUI with a calendar on the left and session details on the right.

## Commands

`cargo` is at `/home/akhil/.cargo/bin/cargo`

```bash
cargo build          # compile
cargo run            # run the tool
cargo clippy         # lint
cargo fmt            # format
```

## Module Architecture

```
src/
├── main.rs       # orchestrates: auth → api → sessions → ui
├── auth.rs       # reads ~/.claude/.credentials.json for plan; persists sessionKey to ~/.config/claudecalender/session.json
├── api.rs        # GET https://claude.ai/api/account with sessionKey cookie; derives billing period from created_at
├── sessions.rs   # scans ~/.claude/projects/*/*.jsonl; aggregates token stats per session
├── models.rs     # shared structs: Credentials, Session, ModelUsage, SessionStats, Output
└── ui.rs         # ratatui TUI: App state + calendar panel (left) + session detail panel (right)
```

## Key Design Decisions

- **Plan** comes from `~/.claude/.credentials.json` (`claudeAiOauth.subscriptionType`) — no API call needed.
- **Billing period** is derived from the `created_at` field of `claude.ai/api/account`. The day-of-month of `created_at` is treated as the monthly billing day.
- **Session data** lives in `~/.claude/projects/{encoded-path}/{sessionId}.jsonl`. The directory encoding replaces `/` with `-` (e.g. `-home-akhil-code-myproject`). Sessions are filtered by the first top-level `timestamp` in the JSONL relative to billing period start.
- **Working directory** comes from the first top-level `cwd` field in the JSONL, not from decoding the directory name — that encoding is lossy for paths containing `-` (e.g. `jnana-bharathi`). Later lines can carry a different `cwd` if the agent `cd`'d mid-session, so only the first is used. Stub sessions with no turns have no `cwd`, but they also have no usage and are skipped.
- **Resume**: `Enter` in the detail panel tears down the TUI and runs `claude --resume <sessionId>` with `current_dir` set to the session's `cwd`.
- **Authentication**: `claude.ai/api/account` requires a browser `sessionKey` cookie, not the OAuth access token from `.credentials.json`. The OAuth token only works for inference endpoints on `api.anthropic.com`. On 401/403, the tool prompts for a new sessionKey and overwrites the saved one.
- **Context utilization** = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` for the turn with the highest total, divided by the model's context limit (200k for all current models, 100k for claude-2/instant).
- **Cache efficiency** = `cache_read / (cache_read + cache_creation) × 100` per model per session.
- **Cost** is computed locally using hardcoded per-million-token prices: Opus $5/$25, Sonnet $3/$15, Haiku $1/$5 (input/output); cache writes at 1.25× base input, cache reads at 0.1× base input.

## UI Layout (ui.rs)

Two-panel split: 20-col calendar on the left, session detail on the right (5-col left padding).

```
      May 2026             Tuesday, May 12 2026
Mo Tu We Th Fr Sa Su       Tokens: 12345  Cost: $0.42
 1  2  3  4  5  6  7
...                        1. /home/user/code/project
                              10:32 – 11:15  (43 min)
Total: $1.23                  Input: 107  Output: 50625
                              Peak ctx: 33.6%
← → ↑ ↓  h j k l  q quit     Cache eff — sonnet-4-6: 96%
```

### Calendar rules
- **Subscription days**: default color.
- **Out-of-period days**: dark gray.
- **Active day**: cyan background, bold.
- **Today** (when not active): yellow, bold.
- **Days with usage**: green gradient background (4 levels, GitHub contribution graph style) based on `(tokens * 4) / max_daily_tokens`.
- **Overflow month** (when billing period spills into a second calendar month): rendered inline in the same grid (no second header), shown in light gray.
- **End-month truncation**: only the week containing `billing_end - 1` is shown in the overflow month; subsequent weeks are omitted.

### Navigation and focus
The UI has two focus modes tracked by the `Focus` enum in `ui.rs`: `Calendar` (default) and `Detail`.

- Calendar focus: `←`/`h`, `→`/`l` move ±1 day; `↑`/`k`, `↓`/`j` move ±7 days; `Enter` shifts focus to Detail; `Esc`/`q` quit.
- Detail focus: `↑`/`k`, `↓`/`j` select a session; `Enter` resumes the selected session; `Esc` returns to Calendar; `q` quits.
- Navigation bounds: `nav_start` = first day of the month containing `billing_start`; `nav_end` = end of the Sunday-terminated week containing `last_valid_day`, capped at month end.
- Active day initialises to today, clamped to `[billing_start, last_valid_day]`.
- Active-day calendar highlight is **cyan** when Calendar is focused, **blue** when Detail is focused.

### Detail panel (right)
- Shows sessions for the active day, newest-first, with project path, time range, duration, input/output tokens, peak context %, and per-model cache efficiency.
- Day summary line (tokens + cost) shown above the session list.
- Add new tabs by incrementing `TAB_COUNT` in `next_tab()` and adding a match arm in `draw_sessions`.
