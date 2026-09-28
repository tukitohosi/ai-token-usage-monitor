# AI Token Usage Monitor

[简体中文](README.md) | English

## Overview

AI Token Usage Monitor is a local-first Windows desktop app. It displays Codex account limits and account-level daily activity separately from token activity found in local Codex, Claude Code, OpenCode, WorkBuddy, and WorkBuddy AI records. Local activity is never presented as a complete account bill or quota.

The current release is **v0.9.6**.

## Highlights

- Explore local activity by AI source, model, project, and date range; inspect Codex account limits separately.
- Choose automatic account sync or a persistent local-only mode. Local token and cost statistics remain available when account reads fail.
- Enter your own model prices. Missing rates stay unpriced, and edits survive refreshes and page changes until you save or discard them.
- Configure multiple weekly peak-pricing windows for selected models. Exclude published mainland China holidays for 2024–2026 and add special dates that always use standard pricing.
- Choose local, Beijing, US Pacific, London, Tokyo, or another IANA city time zone for peak rules. Daylight saving time is applied where relevant.
- Review source health and export privacy-preserving CSV or JSON summaries.

## Install

1. Open [Releases](https://github.com/tukitohosi/ai-token-usage-monitor/releases).
2. Download `AI-Token-Usage-Monitor-0.9.6-Setup.exe` from the v0.9.6 release.
3. Run the installer and follow the prompts.

The installer is not commercially code-signed, so Windows may show an unknown-publisher warning.

## Basic use

1. Start the app and wait for the first local index to finish. A large history may take tens of seconds.
2. Use **Overview** for Codex account limits and recent account activity.
3. Use **Local Activity** for records attributable to this computer.
4. Use **Cost** for estimates based on the prices you entered. Unpriced records are shown explicitly.
5. Use **Model Pricing** to enter rates and configure weekly peak rules, time zones, holidays, and special dates.
6. Use **Settings** for local-only mode, refresh, appearance, notifications, tray behavior, and project grouping.
7. Closing the main window keeps the app in the system tray by default. Choose **Exit** from the tray menu to stop it.

## Data and privacy

- The app does not read Codex `auth.json` or store access tokens.
- Prompts, response bodies, and tool output are not persisted.
- The local index stores derived usage information only; it does not represent activity from other computers or cloud tasks.
- Settings and derived indexes stay in the current Windows user's application-data directory. Maintenance actions do not rewrite source logs.

## Run from source

You need Node.js 22 or newer, Rust, and a Windows build environment.

```powershell
npm install
npm test
npm run typecheck
npm run desktop:dev
```

Build the Windows installer with `npm run desktop:build`.

## Release status

Installers are distributed through GitHub Releases; historical installers are kept locally. Automated checks and a local build do not replace clean-Windows testing for first install, upgrade, uninstall, data retention, and missing-WebView2 behavior.
