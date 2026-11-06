# AI Token Usage Monitor

[简体中文](README.md) | English

A Windows app for viewing local AI token usage, model activity and estimated costs alongside Codex account limits. It reads local records from Codex, Claude Code, OpenCode, WorkBuddy and WorkBuddy AI.

The current release is **v0.10.0**. [Download the installer](https://github.com/tukitohosi/ai-token-usage-monitor/releases/tag/v0.10.0) · [v0.10.0 upgrade notes](docs/v0.10.0-升级说明.md)

## Features

- Explore usage by date, AI source, model and project, with daily details.
- Read plain, Zstandard-compressed and archived Codex logs. Keep recorded history when source files move or disappear.
- Merge Claude Code streaming usage snapshots by reply, including other models used through Claude Code, so each reply is counted once.
- See daily trends and the past 24 hours of activity.
- Refresh local usage and Codex account data in parallel. The refresh indicator ends after both return this cycle's result; local statistics remain available when account sync fails or the account is signed out.
- Enter input, cache-read, cache-write and output prices for each model, with optional peak-pricing windows, multipliers, time zones and special dates.
- Export saved model pricing and peak rules as JSON, or import pricing by case-insensitive modelId into an editable draft before saving and recalculating.
- Model Pricing lists models by all-time local token usage across all sources, highest first. It checks the order after each refresh and when the app opens. Rows stay in place while you type, then reorder when focus leaves the field; refreshes preserve unsaved edits.
- Export model totals, daily trends, daily model usage, source totals and project totals as CSV or JSON from Local Activity or Cost. A complete JSON report includes all five datasets.
- Choose today, the past 7 or 30 days, all history or custom dates, with an independent source selection. Reuse the same captured report for multiple exports; regenerate it when you want updated data.
- Import a custom desktop pet from local Codex files or a supported pet file. Move it and choose its size and theme.
- View today's tokens from all local sources and currency-separated API cost estimates below the pet, plus remaining Codex 5-hour, 7-day and reserve quota with reset times. Old data is labeled; tokens per second are not shown.
- Use local-only mode, themes, the system tray and usage notifications. The pet keeps running when the main window is hidden to the tray; exiting the app stops both.

## Install and use

1. Download AI-Token-Usage-Monitor-0.10.0-Setup.exe from the [v0.10.0 release](https://github.com/tukitohosi/ai-token-usage-monitor/releases/tag/v0.10.0).
2. Start the app and wait for the first local scan. A large history may take tens of seconds.
3. Open Local Activity for usage, Model Pricing for rates or a pricing JSON import, and Cost for estimates.
4. In Desktop Pet settings, import a custom local Codex pet and enable it. The transparent area around the pet window passes clicks through and does not take focus.
5. Enable local-only mode in Settings if you do not need account synchronization.
6. Closing the main window keeps the app in the tray by default. Choose Exit from the tray to stop the app and pet.

## Data and limitations

- Local statistics include previously recorded history and readable records on this computer, not the complete history of other devices or cloud tasks.
- Codex account limits require a network connection. Limit percentages cannot be converted into a fixed token balance.
- Costs use your own prices and are estimates, not provider bills. USD and CNY totals stay separate.
- Pricing JSON uses per-million-token rates. null means unpriced; zero means free. Imports are limited to 1 MiB and 200 models, matched by modelId without regard to case to avoid applying rates to models with the same display name.
- The app imports custom Codex pets available locally or supported pet files. Cloud resources that provide only an ID must first be saved locally. Official built-in Codex pets are not included.
- Cursor does not yet have a connected usage source.
- Built-in mainland China holiday dates cover 2024–2026; use special dates for other years.
- The statistics database does not store access tokens, prompts, responses or tool output. Report exports contain summaries rather than credentials, conversations, full paths or session identifiers.

Settings and derived indexes stay in the current Windows user's application-data directory. Index upgrades and maintenance back up their own indexes. Rebuilding preserves recorded history and does not rewrite source records.

## Run from source

Requires Node.js 22.15 or newer, Rust and a Windows build environment.

~~~powershell
npm install
npm run desktop:dev
~~~

[Architecture](docs/ARCHITECTURE.md) · [Data sources](docs/MULTISOURCE_USAGE.md)