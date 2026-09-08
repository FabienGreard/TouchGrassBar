---
name: convex-deploy-guard
description: Identify Convex deployment targets and apply the authorized task scope before production actions. Use for deployment, environment changes, and production audits.
---

# Deployment target guard

Identify the target, apply the user's existing authorization, then act.

## Workflow

1. Read the selected deployment from the command's environment, root `.env.local`, and `convex.json`, or use the official MCP `status` tool. Check deploy-key presence without printing its value. Classify the target as local, dev, preview, or production. Resolve conflicting sources before proceeding. Completion: the exact target is known.
2. Announce the target before an operation that can change it. Use a short statement such as `target: production (next-pig-820)`. Completion: the user can see where the operation will run.
3. Apply authorization from the conversation. For app releases, follow the [release authorization contract](../release/SKILL.md#authorization); it covers the required production steps. For other tasks, carry prior authorization through the necessary actions and retries for that target. Ask only when the requested production action is outside that scope. Completion: the operation is covered by existing or newly supplied authorization.
4. Match access to the task. Use local development by default. For a production MCP audit, enable only read access with `--cautiously-allow-production-pii`. Enable `--dangerously-enable-production-deployments` only for an authorized production change. A read-only task stays read-only; disable `run,envSet,envRemove` for that task. Completion: tool access matches the authorized scope.
5. Verify the result on the selected deployment. If the expected change is absent, check the target and failure evidence before retrying. Completion: report the verified result or the concrete blocker.
