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

Apple security identity and worktree state are separate concerns. All local
worktrees use the shared Dev app identity `app.touchgrass.bar.dev` and the
installed Dev provisioning profile. The development launcher builds a real
`.app`, embeds that profile, signs the complete bundle, and verifies the bundle
before it starts the executable. This gives the app the entitlement context that
the Data Protection Keychain requires. Production uses the separate
`app.touchgrass.bar` identity and its release signing process.

The shared Dev app identity does not mean shared application data. Each
worktree derives one stable local namespace from its path. The namespace
selects that worktree's SQLite directory, non-synchronizing Data Protection
Keychain service, Convex deployment, browser-preview port, and visible Dev
label. Dev mode also disables the production single-instance, updater, and
launch-at-login plugins so parallel worktrees do not block each other. A
worktree must not add a new Apple bundle identity only to isolate local state.
