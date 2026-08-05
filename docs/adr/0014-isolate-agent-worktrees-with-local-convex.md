# Isolate agent worktrees with local Convex

Each agent worktree uses one anonymous local Convex deployment, with state and
generated environment values stored only in the worktree's ignored `.convex/`
and `.env.local` files. Repository setup creates a private local Better Auth
secret, maps local backend URLs into the native build, and makes the default
Convex development command select only that local deployment.

The personal cloud dev deployment is shared developer state and is not an
agent sandbox. An agent must not select it as a fallback when local setup is
missing or fails. Cloud dev and production actions require an explicit target
and human authorization.

This follows the
[Convex Agent Mode local-backend guidance](https://docs.convex.dev/cli/agent-mode#local-backend).
It gives each worktree isolated data and functions without a Convex login or
deploy key. Local backends have no public URL and must remain active during app
tests, so webhook and production-readiness evidence still require separate
cloud environments and authority.
