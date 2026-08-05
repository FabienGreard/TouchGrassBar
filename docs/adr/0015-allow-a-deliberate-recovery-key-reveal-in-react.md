---
status: accepted
---

# Allow a deliberate Recovery Key reveal in React

This decision supersedes ADR 0004 only for the stored Recovery Key display in Profile Settings. Keychain remains the source of truth, but the narrow reveal command may return the Recovery Key to volatile React component state after the Tokenmaxxer selects **View** so the existing field can reveal its value in place. React clears its reference on **Hide**, section or window changes, and unmount; the value must not enter Sanitized Desktop State, persistence, logs, synchronization payloads, previews, or evidence. The initial creation disclosure remains a native secure sheet. This accepts that JavaScript strings cannot be reliably zeroized and can remain in WebView memory until garbage collection in exchange for the consistent inline settings interaction.
