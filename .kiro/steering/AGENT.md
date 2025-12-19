# DESCARTES monorepo

## Clippy

Run clippy occasionally to fix lint issues

## Issue Tracking with Beads

IMPORTANT: Use `bd` (beads) for ALL issue tracking, epics, and task management. Do NOT use markdown files, TODO comments, or TodoWrite for tracking work.

```
bd init                               # Initialize beads (first time only)
bd prime                              # Load context at session start
bd ready                              # Find work with no blockers
bd create --title="..." --type=task   # Create new issue
bd update <id> --status=in_progress   # Claim work
bd close <id>                         # Mark complete (use <id> <id2> for multiple)
bd sync --flush-only                  # Export before commits
```

Run `bd init` if `.beads/` doesn't exist, then `bd prime` at each session start. 