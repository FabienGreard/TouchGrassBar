# Use invisible generated credentials

TouchGrassBar uses Better Auth with a permanent server-generated TouchGrass ID and a high-entropy generated Recovery Key instead of conventional sign-up. Rust stores the recovery credential and session material in macOS Keychain and authenticates synchronization without exposing either to React; this preserves a nearly zero-friction first run while retaining hashed credentials, revocable sessions, and cross-device recovery.
