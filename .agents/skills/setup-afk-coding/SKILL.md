---
name: setup-afk-coding
description: Install or update an hourly Codex automation that wakes PR workers and dispatches ready GitHub tickets into isolated AFK coding tasks.
---

# Setup AFK Coding

Install one hourly control tower for this repository. The control tower orchestrates; `xhigh` worker tasks implement.

## 1. Resolve the installation

Find the automation tool before taking action. Resolve the current repository's saved Codex project and confirm it is a Git repository. Inspect existing automations before creating one; match by project and the exact name `<repository name> AFK Coding` or by a prompt marker beginning `afk-coding-contract:`.

Read [orchestrator-contract.md](references/orchestrator-contract.md), [worker-contract.md](references/worker-contract.md), and [testing-contract.md](references/testing-contract.md) completely. Compute one SHA-256 over their exact UTF-8 bytes in that order, with one newline between adjacent files.

Completion criterion: the saved project, zero or one matching automation, and the exact contract hash are known. Resolve duplicates explicitly before continuing.

## 2. Prepare the issue tracker

Use the tracker conventions in `docs/agents/issue-tracker.md`. Ensure these operational labels exist without changing ticket state:

| Label | Color | Meaning |
| --- | --- | --- |
| `agent:claimed` | `1D76DB` | An AFK worker owns the issue |
| `agent:blocked` | `D93F0B` | The worker needs external or human action |
| `agent:stale-claim` | `FBCA04` | A dispatcher lease may be abandoned |
| `review:ready` | `0E8A16` | A verified PR awaits human review |

Inspect protection on the default branch. Record whether it requires pull requests, the repository CI check, human approval, and resolved conversations. This is a runtime dispatch gate, not an installation gate: install the automation even when protection is incomplete, and report that it will reconcile existing PRs but create no new workers until the gate passes.

Completion criterion: all four labels exist with the intended meanings, and branch protection has a named pass/fail result.

## 3. Compile the automation prompt

Build the prompt from this header followed by the three full contracts; preserve the source text instead of paraphrasing it:

```text
afk-coding-contract: v2
contract-sha256: <computed hash>
orchestrator-reasoning: medium
worker-reasoning: xhigh

Execute one hourly AFK coding orchestration cycle for this project. The following contracts are authoritative for this run.

## Orchestrator Contract
<full orchestrator-contract.md>

## Embedded Worker Contract
<full worker-contract.md>

## Embedded Testing Contract
<full testing-contract.md>
```

The hash makes drift observable while the embedded text lets a run operate without depending on the setup skill being loaded.

Completion criterion: the compiled prompt contains all three full contracts once, the correct hash, `medium` orchestrator reasoning, and `xhigh` worker reasoning.

## 4. Install idempotently

Create or update exactly one standalone project automation with:

- name `<repository name> AFK Coding`;
- active status and an hourly schedule;
- the resolved project;
- worktree execution and destination;
- the user's configured default model;
- `medium` reasoning effort;
- the compiled prompt.

Use the automation tool rather than writing automation files. Preserve the existing automation id, notification policy, and fields not owned above. Let the application choose its default notification policy for a new automation. Update a matching automation when its schedule, environment, reasoning, status, or contract hash drifts; create only when none exists.

Completion criterion: the automation tool reports one active matching automation and no duplicate was created.

## 5. Verify the installed control tower

Read the installed automation back through the automation tool. Confirm project, hourly cadence, active status, worktree isolation, `medium` reasoning, prompt hash, and all three embedded contracts. Report the automation id, first eligible run behavior, worker cap, `xhigh` worker setting, minimum-sufficient-proof policy, notification policy, and branch-protection gate.

The setup is complete only after read-back matches the compiled contract. Do not dispatch a worker directly from this setup run; the hourly automation owns dispatch.
