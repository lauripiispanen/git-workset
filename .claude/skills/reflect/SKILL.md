---
name: reflect
description: End-of-task reflection to capture learnings and improve automation
argument-hint: "[task-summary]"
---

# Post-Task Reflection

Run this after completing a significant task to capture learnings and improve future work.

## Step 1: Identify Mistakes

Ask yourself: **What mistakes did I make that could be prevented with advance information?**

Consider:
- Repeated iterations to fix linter/clippy warnings
- Wrong approaches that required backtracking
- Misunderstandings about project conventions
- Did the user need to correct me or tell me to do something I should've done proactively

For each mistake, determine:
- What information would have prevented it?
- Should this be added to agents.md? We also want to avoid bloating it.

## Step 2: Review Validation Gaps

Ask yourself: **Are there checks I ran manually that should be automated?**

Consider:
- Commands run to verify correctness (type-checks, lints, tests)
- Manual validations that caught issues
- Patterns that should always be enforced

For each gap, determine:
- Should this be a pre-commit hook? (runs on every commit)
- Should this be a stop hook? (runs when Claude finishes a turn)

## Step 3: Update Documentation

If learnings were identified in Step 1:
1. Read agents.md
2. Add concise guidance to the appropriate section
3. Keep additions minimal - one line per learning if possible

## Step 4: Update Hooks

If validation gaps were identified in Step 2:
1. Read `.claude/hooks/pre-commit-check.sh`
2. Add new checks following the existing pattern
3. Test the hook manually before committing

## Step 5: Commit Changes

If agents.md or hooks were updated:
1. Stage the changed files
2. Commit with message: "Add learnings from [task-summary]: [brief description]"
(3. Prefer to amend the feature commit)

## Output Format

Summarize what was done:

```
## Reflection Summary

### Learnings Added to agents.md
- [learning 1]
- [learning 2]

### Hooks Added/Updated
- [hook change 1]
- [hook change 2]

### No Changes Needed
- [reason if nothing was added]
```

---

Task context: $ARGUMENTS
