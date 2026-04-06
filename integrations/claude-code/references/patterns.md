# Orchestration Patterns

## Spawn a worker in another repo

1. `yatamux list-panes --json`
2. `pwsh -File integrations/claude-code/scripts/new-worker-pane.ps1 -Alias docs -Role writer -Dir C:\src\other-repo`
3. `yatamux send-keys --pane docs --enter --raw "claude --continue"`
4. `yatamux send-keys --pane docs --enter --raw "Draft the release notes and leave the final summary in the pane output."`
5. `pwsh -File integrations/claude-code/scripts/watch-pane.ps1 -Pane docs -Json`

## Use exec for bounded commands

For single commands where exit code matters, prefer:

`yatamux exec --pane tests --wait-for output-regex --output-regex "test result: ok" -- cargo test`

This is safer than raw `send-keys` when you want one request to include send + wait + exit propagation.

## Recover a stuck worker

1. `yatamux capture-pane --target worker-a --lines 200 --plain-text`
2. `yatamux interrupt-pane --pane worker-a`
3. If the worker still does not respond, `yatamux terminate-pane --pane worker-a`
4. If the pane is no longer needed, `yatamux close-pane --pane worker-a`

## Suggested prompt snippet

Use the following style when instructing Claude Code:

`Create a dedicated yatamux worker pane for this task, label it, run the work there, monitor it with subscribe-pane, and summarize the result back in the main pane when finished.`
