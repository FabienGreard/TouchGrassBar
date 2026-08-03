# Native core owns provider integrations

The Rust core owns provider detection, local usage parsing, Keychain access, caching, background refresh, and Convex synchronization; React receives only sanitized application data. This concentrates privileged access and privacy enforcement behind the Tauri command boundary instead of allowing webview code to read provider material directly.
