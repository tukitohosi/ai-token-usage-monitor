# AI Token Usage Monitor

[简体中文](README.md) | English

## Overview

AI Token Usage Monitor is a local-first Windows desktop app that brings together Codex account-level limits and daily token activity with activity that can be derived from local Codex, Claude Code, OpenCode, WorkBuddy, and WorkBuddy AI records. Account data and local-machine data are always shown separately, so activity visible on one computer is never presented as a complete account bill or quota.

The current release is **v0.9.2**.

## Highlights

- View Codex account limits and recent account-level daily token activity.
- Explore local activity by AI source, model, project, and time range.
- Filter by today, 7 days, 30 days, all time, or a custom date range.
- Switch between single-source and multi-source views.
- Review cost estimates based on an offline public pricing snapshot; unpriced or incomplete records are never reported as zero cost.
- Configure refresh intervals, themes, notifications, tray behavior, and local project merge rules.
- Inspect source health and export privacy-preserving CSV or JSON summaries.

## Install

1. Open [Releases](https://github.com/LQ-FJUT/ai-token-usage-monitor/releases).
2. Download `AI Token 用量监控-0.9.2-Setup.exe`.
3. Run the installer and follow the prompts.

The installer is not commercially code-signed, so Windows SmartScreen may show an unknown-publisher warning. Check the SHA-256 value published on the Release page before deciding whether to run it.

## Basic use

1. Start the app and wait for the first local index to finish. A large history may take tens of seconds.
2. Use **Overview** to inspect Codex account limits and recent account activity.
3. Use **Local Activity** to explore data attributable to this computer.
4. Apply a date range and select one or multiple AI sources.
5. Use **Cost** for clearly qualified estimates based on the bundled offline price snapshot.
6. Use **Settings** to adjust refresh, appearance, notifications, tray behavior, and project grouping.
7. Closing the main window keeps the app in the system tray by default. Choose **Exit** from the tray menu to stop it completely.

## Data and privacy

- The app does not read Codex `auth.json` and does not store access tokens.
- Prompts, response bodies, and tool output are not persisted.
- The local index stores derived usage information only. It does not represent activity from other computers or cloud tasks.
- Settings and derived indexes stay in the current Windows user's application-data directory.
- Maintenance actions do not rewrite the original AI session logs.

## Run from source

You need Node.js 22 or newer, the Rust toolchain, and a Windows build environment.

```powershell
npm install
npm test
npm run typecheck
npm run desktop:dev
```

Build the Windows installer:

```powershell
npm run desktop:build
```

## Release status

This repository contains the final source for v0.9.2, while installers are distributed through GitHub Releases. A successful local build and automated checks do not replace clean-Windows acceptance testing for first install, upgrade, uninstall, data retention, and WebView2 fallback behavior.
