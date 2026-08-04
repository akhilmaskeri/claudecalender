# claude calender

A Rust CLI tool that displays Claude Code usage for the current subscription period with an interactive terminal UI.

## Features

- Reads your plan and billing cycle from `~/.claude/.credentials.json` and the claude.ai API
- Scans all local Claude Code sessions since the billing period start date
- Interactive TUI with a calendar view and per-session stats
- Navigate by day with arrow keys; session details update in real time

## Authentication

On first run the tool prompts for your `sessionKey` cookie from claude.ai (one-time setup):

1. Open `https://claude.ai` in your browser and log in
2. Open DevTools (`F12`) → **Application** → **Cookies** → `https://claude.ai`
3. Copy the value of the `sessionKey` cookie and paste it when prompted

The session is saved to `~/.config/claudecalender/session.json` for subsequent runs. If it expires, the tool detects the 401 and prompts again.

## Build

```bash
cargo build
```

## Run

```bash
cargo run
```

## UI

The tool opens an interactive full-screen terminal UI split into two panels:

```
      May 2026             Tuesday, May 12 2026
Mo Tu We Th Fr Sa Su       3 days remaining in billing period
             1  2  3
 4  5  6  7  8  9 10       1. /home/user/code/myproject
11 12 13 14 15 16 17          10:32 – 11:15  (43 min)
18 19 20 21 22 23 24          Input: 107  Output: 50625
25 26 27 28 29 30 31          Peak ctx: 33.6%
 1  2  3  4  5  6  7          Cache eff — sonnet-4-6: 96%
```

**Calendar panel (left)**

- Displays the billing period as a single continuous calendar
- If the billing period spans two calendar months, overflow dates continue in the same grid (no second header) and are shown in light gray
- Days outside the subscription period are shown in dark gray
- Days with sessions are colored on a green gradient based on total token usage (GitHub contribution graph style) — more tokens = brighter green
- The active day has a cyan background highlight
- Today (when not the active day) is shown in yellow

**Session panel (right)**

- Lists all sessions for the active day, sorted newest-first, with project path, time range and duration, input/output token counts, peak context utilization, and per-model cache efficiency

**Navigation**

The UI has two focus modes: **Calendar** (default) and **Detail**.

| Key | Context | Action |
|-----|---------|--------|
| `←` / `h` | Calendar | Move active day back one day |
| `→` / `l` | Calendar | Move active day forward one day |
| `↑` / `k` | Calendar | Move active day back one week |
| `↓` / `j` | Calendar | Move active day forward one week |
| `Enter` | Calendar | Shift focus to detail panel |
| `Tab` | Detail | Cycle through detail content |
| `Esc` | Detail | Return focus to calendar |
| `q` | Any | Quit |
| `Esc` | Calendar | Quit |

When the detail panel has focus, the active day in the calendar turns blue (vs cyan when calendar is focused).

## Field reference

| Field | Description |
|-------|-------------|
| `duration_mins` | Wall-clock time from first to last message in the session |
| `peak_context_pct` | Largest context window used in a single turn as % of the 200k limit |
| `cache_eff` | % of input tokens served from cache per model (higher = less reprocessing) |
| `input_tokens` | Fresh (non-cached) input tokens across all turns, summed per session |
| `output_tokens` | Output tokens generated across all turns |

Sessions are filtered to the current billing period.
