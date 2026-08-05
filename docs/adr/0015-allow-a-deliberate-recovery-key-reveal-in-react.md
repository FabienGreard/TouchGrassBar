---
status: accepted
---

# Allow a deliberate Recovery Key reveal in React

This decision supersedes ADR 0004 only for the stored Recovery Key display in Profile Settings. Keychain remains the source of truth. Settings State may contain only the real final three Recovery Key characters so the masked field identifies the stored key without inventing a format. The narrow reveal command may return the full Recovery Key to volatile React component state after the Tokenmaxxer selects **View** so the existing field can reveal and select its value in place. React clears its full-key reference on **Hide**, section or window changes, and unmount; the full value must not enter Sanitized Desktop State, persistence, logs, synchronization payloads, previews, or evidence. Onboarding does not display the Recovery Key and closes when the Profile is Ready. This accepts that JavaScript strings cannot be reliably zeroized and can remain in WebView memory until garbage collection in exchange for the consistent inline settings interaction.
