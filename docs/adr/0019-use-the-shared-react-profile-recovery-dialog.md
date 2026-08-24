---
status: accepted
---

# Use the Shared React Profile Recovery Dialog

Profile recovery uses the existing branded React dialog in onboarding and
Settings. The dialog holds the entered TouchGrass ID and Recovery Key only in
volatile component state and sends both values through one narrow typed Tauri
command when the Tokenmaxxer selects **Recover Profile**. Rust performs all
validation, transport, retry, Keychain, session, and Active Mac authority work.

This decision adds one deliberate exception to the native-owned credential
boundary in ADR 0004. The Recovery Key must not enter Sanitized Desktop State,
persistence, logs, synchronization payloads, previews, or evidence. React
clears the input when the recovery attempt succeeds, the Tokenmaxxer cancels,
or the dialog unmounts. The app does not add a second AppKit recovery sheet or
a native preview wrapper. This accepts that a JavaScript string cannot be
reliably zeroized in exchange for one consistent product dialog.
