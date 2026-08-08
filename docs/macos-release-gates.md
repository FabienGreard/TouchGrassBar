# macOS release gates

This runbook owns the durable QA procedure from issue #35. Use it for the
exact signed and notarized draft candidate from the [release
runbook](release.md). A development build, a rebuilt app, or one sample is not
release evidence.

## Stop rules

Stop the run when one of these conditions is true:

- The DMG, embedded app, version, commit, or artifact SHA-256 does not match
  the draft candidate.
- The test Mac is not an M4 Pro Mac with 24 GiB of memory.
- AC power is not connected or Low Power Mode is on.
- A live provider or network response enters a measured interval.
- A required sample, binding, automated result, or physical result is absent.
- A median, worst, artifact-size, refresh, or recovery limit fails.

There is no waiver. A code or bundle change makes the receipt invalid. Make a
new candidate. Run all gates again. You can correct evidence text without a new
run only when the artifact SHA-256 does not change.

## Evidence boundary

The receipt can contain these items:

- Schema version, candidate version, commit, and artifact SHA-256.
- Byte sizes.
- Hardware model, chip, memory, power state, and macOS version.
- Fixture version, fixture SHA-256, and fixture byte size.
- Raw samples and computed results.
- Closed PASS or FAIL states.

Do not put these items in the receipt, an issue, a pull request, or a
recording:

- Credentials, sessions, or recovery material.
- Provider source data or raw logs.
- Local paths or private identifiers.

Keep the generated fixture and intermediate files in a temporary directory
outside the repository.

## Prepare the candidate and fixture

1. Get the exact final DMG and its `release-trust-VERSION.json` file from the
   same governed draft Release. Do not rebuild the DMG. Do not re-sign it. Do
   not repackage it. Do not staple it.
2. Check that the trust receipt has the stable `MAJOR.MINOR.PATCH` version and
   the exact 40-character lowercase commit. Check that it has the 64-character
   lowercase SHA-256 for the DMG. Do not edit the receipt.
3. On the M4 Pro Mac, record the sanitized hardware model, `Apple M4 Pro`
   chip, 24 GiB memory, and exact macOS version.
4. Connect AC power. Turn off Low Power Mode. Keep these conditions for the
   complete run.
5. Create an inspection copy of the deterministic maximum-size local fixture:

   ```sh
   fixture_directory="$(mktemp -d)"
   bun run macos:refresh-fixture -- "$fixture_directory/refresh-fixture.json"
   ```

The fixture is synthetic and versioned. It contains the product maxima for two
providers. It has 60 Ranking Days and 30 model-cost days for each provider. It
has 100 global rows and 100 My Tokenmaxxers rows. The command always creates the
same bytes for one fixture version. The release-gate harness uses the same
canonical generator and binds the fixture version, SHA-256, and byte size to
its receipt. Do not edit the inspection copy.

## Run the automated gate

Run the harness from the repository root. Set these shell variables to the
exact draft files and a temporary output file outside the repository:

```sh
bun run macos:release-gates -- \
  --dmg "$candidate_dmg" \
  --trust "$release_trust_receipt" \
  --output "$gate_receipt"
```

The harness mounts the DMG read-only and starts the app that is inside it. It
does not accept a replacement app. It writes one sanitized JSON receipt to the
`--output` file. A missing option, an extra option, invalid trust evidence, or
any failed gate gives a nonzero exit status. Accept only a zero exit status and
a top-level PASS result.

The data contract is also strict. Each of these conditions blocks release:

- An error or a FAIL state.
- Missing output or malformed JSON.
- An unknown field.
- A stale schema version or fixture version.
- A non-finite sample or a negative sample.

## Measurement protocol

The harness collects exactly five finite, nonnegative samples for each timed
or process metric. It keeps the raw order and calculates the median and worst
value again. It does not trust a supplied summary.

### Cold startup

Quit the candidate fully before each sample. Start the exact app as a new
process. The sample starts at process launch. It ends only when the menu-bar
item exists and the panel renderer can show cached, loading, stale, or
unavailable state. Provider and network completion cannot delay this endpoint.

### Complete panel paint

The production tray release and the automated driver use the same native
release handler. The driver marks its samples as synthetic, and the receipt
accepts only that marker. A real tray release uses a separate `tray` marker and
cannot enter an automated sample by mistake. Native code shows and positions
the panel, then requests a paint acknowledgement. The renderer waits for fonts,
images, the final native resize, and two complete animation frames. Native code
then confirms a visible display frame before it records the sample. A window
`show` or focus result is not a complete sample. The required physical pass
still checks a real click.

### Idle CPU and settled memory

Idle CPU is the sum for TouchGrassBar and its child WebView or helper process
tree. Close the panel. Make sure that no refresh is active. Wait one minute.
Collect five separate 10-second process-tree averages.

For settled RSS, open the panel one time. Close the panel. Wait one minute. Sum
the RSS of TouchGrassBar and its dedicated WebView or helper processes. Collect
five samples. Do not use only the root process.

### Background refresh

Use only the canonical local fixture. Do not include a live provider request
or network response time. The app loads the fixture through its native refresh
coordinator. It commits the result to the in-memory read model. The panel then
renders the fixture provider data. Use a new app process for each sample. Run
five refresh samples. For each sample, record these measurements:

- Panel paint time during local work.
- Average process-tree CPU.
- Peak process-tree RSS.
- Time until process-tree CPU returns to at most 1 percent.

A missing completion event or a process-tree read error fails the run.

### Limits

| Metric | Median limit | Worst limit |
| --- | ---: | ---: |
| Cold startup | 1,000 ms | 2,000 ms |
| Complete panel paint | 100 ms | 200 ms |
| Idle process-tree CPU | 0.5% | 1% |
| Settled process-tree RSS | 200 MB | 250 MB |

The signed app must be at most 40,000,000 bytes. The final DMG must be at most
25,000,000 bytes. These values use decimal MB limits.

During local refresh, every panel-paint sample must be at most 200 ms. Every
average CPU sample must be at most 25 percent. Every peak RSS sample must be at
most 250 MB. CPU must recover to at most 1 percent within 5,000 ms in every
sample.

## Receipt acceptance

Accept a receipt only when all of these facts are true:

- The schema is `touchgrass.macos-release-gates.v1`.
- Candidate version, commit, artifact SHA-256, app bytes, and DMG bytes match
  the exact draft.
- Hardware model, `Apple M4 Pro` chip, 24 GiB memory, AC power, Low Power Mode
  off, and the exact macOS version are present.
- Fixture version, SHA-256, and bytes match the canonical local fixture.
- Each metric contains exactly five raw samples plus the recomputed median,
  worst, and PASS state.
- Positioning and clamping, toggling, Escape, outside click, rapid interaction,
  persisted launch at login, current Space, macOS 15 floor, and latest-stable
  automated preflight states are PASS.
- The top-level state is PASS.

The harness runs named native tests for positioning and current Space. It runs
the named panel keyboard test for Escape. The embedded release driver checks
toggling, outside-click dismissal, and rapid interaction. It also enables,
disables, and restores launch at login in the isolated harness home.

Store the sanitized receipt with the draft candidate evidence. The receipt
does not replace the physical pass. Link the green exact-commit CI run with
this evidence.

## CI operating-system preflight

The current CI native matrix runs on the `macos-15` floor and the `macos-26`
latest-stable runner. Both matrix entries must pass for the exact candidate
commit. The matrix uses `fail-fast: false`, so one result cannot hide the other.

At candidate freeze, confirm that macOS 26 is still the newest stable macOS. If
Apple has released a newer stable version, update the CI matrix. Make sure that
the updated matrix passes before release. CI proves automated native checks. It
does not prove reference Mac performance or physical macOS behavior.

## Required physical pass

Use the exact candidate artifact on one physical supported Apple-silicon Mac.
Record its exact model and macOS version. Complete this binary checklist after
the automated receipt passes:

- One menu-bar item appears and no Dock icon appears.
- A real click opens one panel at the correct anchor on the built-in display.
- The panel has the correct anchor on one extended display and stays usable
  after that display is disconnected.
- The panel opens on the current Space.
- The panel opens over a fullscreen app without leaving fullscreen or changing
  Space.
- Enabled launch at login starts exactly one menu-bar-only instance and no
  window. Disabled launch at login prevents the next sign-in launch. Manual
  launch still works.
- VoiceOver identifies the menu-bar item and all controls.
- Keyboard focus and activation work, and Escape closes the panel.
- The app does not request Accessibility permission.
- Reduce Motion removes no information or action.
- Increase Contrast keeps content, controls, states, and focus distinct.

Space, fullscreen, real sign-in, display, and VoiceOver behavior are
physical-only release gates. Automated positioning, state, and accessibility
checks do not replace them. Attach one short sanitized recording. You can use a
second recording for the sign-in checks. Any FAIL or absent result blocks
release.

One physical Mac is sufficient. This contract does not require every display
layout, a clamshell test, or a manual test on each supported macOS version.
