---
name: claude-code-yatamux
description: Use when Claude Code should orchestrate work across yatamux panes, create labeled worker panes, send instructions, monitor live output, and recover or close workers safely.
---

# Claude Code + yatamux

Use this skill when Claude Code needs to fan work out into separate terminal panes instead of mixing multiple jobs into one shell.

## Core workflow

1. Inspect the current session before sending input.
   Run `yatamux list-panes --json` and choose an existing pane by `alias` / `role` when possible.
2. Create a dedicated worker pane for each isolated task.
   Prefer `integrations/claude-code/scripts/new-worker-pane.ps1` so the pane is created, labeled, and optionally bootstrapped in one step.
3. Label every long-lived worker.
   Use `yatamux set-pane-meta --pane <id> --alias <name> --role <role>` immediately after creation if you did not use the wrapper.
4. Send instructions through yatamux, not by typing into the current pane yourself.
   Use `yatamux send-keys --pane <alias> --enter --raw "<command>"` for interactive bootstraps and `yatamux exec --pane <alias> -- <command>` for one-shot commands with wait conditions.
5. Monitor workers without busy polling.
   Prefer `yatamux subscribe-pane --pane <alias> --json` for live progress. Fall back to `yatamux capture-pane --target <alias> --plain-text` or `--json` when you need a snapshot.
6. Recover safely.
   Use `yatamux interrupt-pane` first, `yatamux terminate-pane` only when the worker is stuck, and `yatamux close-pane` after results are collected.

## Behavioral rules

- One task, one worker pane. Do not multiplex unrelated jobs into the same pane.
- Prefer aliases like `tests`, `docs`, `server`, `worker-a` over raw pane IDs in prompts and scripts.
- When a worker is operating in another repository, pass `--dir` during `split-pane` so the pane starts in the correct working directory.
- If output matters over time, prefer `subscribe-pane` to repeated `capture-pane` calls.
- Before closing or terminating a pane, capture or summarize the result you need.

## Quick commands

- Create a new labeled worker:
  `pwsh -File integrations/claude-code/scripts/new-worker-pane.ps1 -Alias tests -Role verifier -Dir C:\src\repo -BootstrapCommand "codex resume --last"`
- Stream JSON updates:
  `pwsh -File integrations/claude-code/scripts/watch-pane.ps1 -Pane tests -Json`
- Take a snapshot:
  `pwsh -File integrations/claude-code/scripts/watch-pane.ps1 -Pane tests -Snapshot -Lines 200 -Json`

Read `references/patterns.md` for concrete orchestration recipes and prompt snippets.
