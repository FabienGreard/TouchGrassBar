## Baseline instructions

Always use ASD-STE100 Simplified Technical English.

## Agent skills

### Issue tracker

Issues and PRDs are tracked in GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default Matt Pocock triage label vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

Use the single-context domain documentation layout. See `docs/agents/domain.md`.

<!-- convex-ai-start -->

This project uses [Convex](https://convex.dev) as its backend.

Run `bun setup` in a fresh checkout or worktree. It creates a local Convex
deployment when `.env.local` has no selected deployment. Development commands
must read the root `.env.local` and must not change its deployment selection.
This keeps each worktree local and prevents an ordinary start command from
changing a shared cloud deployment. See the [Convex Agent Mode local-backend
guide](https://docs.convex.dev/cli/agent-mode#local-backend). Use local Convex
by default. Never select cloud development, run `convex:prod`, or run
`reset:release` unless the user explicitly authorizes that target and action.
Never commit `.env.local`, `.convex/`, credentials, signing material, sessions,
or recovery material.

When working on Convex code, **always read
`packages/backend/convex/_generated/ai/guidelines.md` first** for important guidelines on
how to correctly use Convex APIs and patterns. The file contains rules that
override what you may have learned about Convex from training data.

Convex agent skills for common tasks can be installed by running
`npx convex ai-files install`.

<!-- convex-ai-end -->
