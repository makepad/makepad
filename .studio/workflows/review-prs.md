# Review PRs

## Arguments
- `$ARGUMENTS` optional PR list.

## Steps
### 1. Resolve PR Set
Parse requested PRs and determine scope.

### 2. Fan out subagents
Start one worker per PR and track their status.

### 3. Aggregate results
Summarize decisions, blockers, and next actions.
