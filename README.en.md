# AI Token Usage Monitor

[简体中文](README.md) | English

A Windows app for viewing AI token usage, model activity and estimated costs alongside Codex account limits. It reads local records from Codex, Claude Code, OpenCode, WorkBuddy and WorkBuddy AI.

The current release is **v0.9.7**. [Download](https://github.com/tukitohosi/ai-token-usage-monitor/releases/tag/v0.9.7)

## Features

- Explore usage by date, AI source, model and project, with daily details.
- See daily trends and the past 24 hours of activity.
- View Codex account limits and account daily activity separately from local records.
- Enter input, cache-read, cache-write and output prices for each model. Missing prices remain unpriced.
- Model Pricing lists models by all-time local token usage across all sources, highest first. It checks the order after each refresh and when the app opens. Rows stay in place while you type, then reorder when focus leaves the field; refreshes preserve unsaved edits.
- Set weekly peak-pricing windows, multipliers, city time zones and special dates for selected models.
- Export model totals, daily trends, daily model usage, source totals and project totals as CSV or JSON from Local Activity or Cost. A complete JSON report includes all five datasets.
- Choose today, the past 7 or 30 days, all history or custom dates, with an independent source selection. Reuse the same captured report for multiple exports; regenerate it when you want updated data.
- Use local-only mode, themes, the system tray and usage notifications.

## Install and use

1. Download AI-Token-Usage-Monitor-0.9.7-Setup.exe from the [v0.9.7 release](https://github.com/tukitohosi/ai-token-usage-monitor/releases/tag/v0.9.7).
2. Start the app and wait for the first local scan. A large history may take tens of seconds.
3. Open Local Activity for usage, Model Pricing for rates and Cost for estimates.
4. Enable local-only mode in Settings if you do not need account synchronization.
5. Closing the window keeps the app in the tray by default. Choose Exit from the tray to stop it.

## Data and limitations

- Local statistics cover readable records on this computer, not the complete history of other devices or cloud tasks.
- Codex account limits require a network connection. Limit percentages cannot be converted into a fixed token balance.
- Costs use your own prices and are estimates, not provider bills. USD and CNY totals stay separate.
- Cursor does not yet have a connected usage source.
- Built-in mainland China holiday dates cover 2024–2026; use special dates for other years.
- The app does not read Codex auth.json or store access tokens, prompts, responses or tool output. Exports contain summaries rather than conversations, full paths or session identifiers.

Settings and derived indexes stay in the current Windows user's application-data directory. Index maintenance backs up its own index and does not rewrite source records.

## Run from source

Requires Node.js 22 or newer, Rust and a Windows build environment.

~~~powershell
npm install
npm run desktop:dev
~~~

[Architecture](docs/ARCHITECTURE.md) · [Data sources](docs/MULTISOURCE_USAGE.md)
