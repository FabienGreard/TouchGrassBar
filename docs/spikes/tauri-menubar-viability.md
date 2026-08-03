# Tauri Menu-Bar Viability Spike

## Outcome

The architecture is viable with a native AppKit adjustment, subject to physical multi-monitor/fullscreen testing and notarizing a real TouchGrassBar artifact.

The disposable implementation lives in `spikes/tauri-menubar`. Proven native behavior has been transferred into `apps/desktop`; the spike itself is not product code.

## Evidence

| Gate | Result | Evidence and boundary |
| --- | --- | --- |
| Menu-bar-only process | Code path proven | Production uses Tauri's Accessory activation policy and hides the Dock icon. Direct visual Dock inspection remains part of device QA. |
| Left-click panel toggle | API and implementation proven | Tauri tray events provide the clicked tray rectangle and the code handles left-button release. The accessibility bridge could not click `SystemUIServer`, so a physical tray click remains device QA. |
| Panel positioning | Automated | Rust tests cover centering, edge clamping, and a monitor with negative coordinates. |
| Escape dismissal | Verified | The smoke harness rendered the panel and an Escape key press hid it. |
| Outside-click dismissal | Verified | Focus changed from true to false after Finder was focused, and the panel hid. |
| Spaces and fullscreen apps | Native path proven; physical QA pending | Tauri's all-workspaces flag is supplemented with AppKit `CanJoinAllSpaces`, `FullScreenAuxiliary`, `Transient`, and `IgnoresCycle`. The pinned native code compiles; behavior across real Spaces and fullscreen apps still needs hands-on testing. |
| Settings window | Implemented | Settings is a separate decorated Tauri window. Onboarding has its own window label. |
| Launch at login | Integrated | The official Tauri autostart plugin is initialized with the macOS LaunchAgent mechanism. End-to-end enable/disable UI is pending. |
| Automatic updates | Integrated foundation | The updater and process plugins are initialized. A signed update manifest, endpoint, and production public key remain unresolved. |
| App bundle | Built | The spike produced an approximately 11 MB macOS `.app` with a roughly 11.2 MB arm64 executable and macOS 14 minimum target. |
| Ad-hoc signing | Verified | The spike app passed strict deep `codesign` verification with an ad-hoc hardened-runtime signature. Gatekeeper rejection is expected for an ad-hoc identity. |
| Developer ID signing | Credential gate proven | A separate timestamped disposable binary was signed and verified using the existing Developer ID Application identity. This was not the TouchGrassBar release artifact. |
| Notarization authentication | Credential gate proven | A dedicated owner-only Team API key authenticated successfully with `notarytool history`. No artifact was submitted. |
| CI signing/notarization | Pending | GitHub Actions secret wiring and a notarized CI-produced TouchGrassBar artifact remain release gates. |

## Performance gates

The spike established an approximately 11 MB app-size baseline. The current production scaffold builds to approximately 15 MB with a 15,251,136-byte arm64 executable. A raw-binary idle sample after 20 seconds reported 0.0% CPU and 39,280 KB resident memory; it also produced LaunchServices warnings because it was outside its app bundle, so this is a diagnostic baseline rather than release evidence. Native setup and tray-toggle paths emit millisecond timing markers. Release candidates must still capture repeated cold startup, first painted panel latency, idle CPU, resident memory, and final DMG size on a real signed build; these cannot be inferred from a Vite smoke harness or a single local sample.

## Decision

Proceed with Tauri. Keep Tauri `2.11.x` pinned while the app uses native `NSWindow` access. Do not mark the native shell release-ready until the remaining physical behavior checks and a real notarization submission pass.
