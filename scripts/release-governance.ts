#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { stableUpdaterEndpoint } from "./release-contract";

const repository = "FabienGreard/TouchGrassBar";
const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const governancePath = resolve(
  workspaceRoot,
  ".github",
  "release-governance.json",
);
const governance = JSON.parse(readFileSync(governancePath, "utf8")) as {
  actions: {
    allowed_actions: "selected";
    github_owned_allowed: boolean;
    patterns_allowed: string[];
    sha_pinning_required: boolean;
    verified_allowed: boolean;
  };
  environments: Record<
    "macos-release" | "public-release",
    {
      deployment_tag_patterns: string[];
      prevent_self_review: boolean;
      required_reviewers: string[];
      secrets: string[];
      variables: string[];
      wait_timer_minutes: number;
    }
  >;
  immutable_releases: boolean;
  reviewer: string;
  tag_ruleset: {
    bypass_actors: unknown[];
    enforcement: "active";
    include: string[];
    rules: Array<"update" | "deletion">;
  };
};

type NameMetadata = { name: string; updatedAt: string };

function command(
  executable: string,
  argumentsList: string[],
  options: { input?: string; silent?: boolean } = {},
) {
  const result = spawnSync(executable, argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    input: options.input,
    stdio: [options.input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    if (!options.silent) {
      throw new Error("The release-governance GitHub operation failed.");
    }
    return null;
  }
  return result.stdout.trim();
}

function ghApi(endpoint: string, method = "GET", body?: unknown) {
  const argumentsList = [
    "api",
    "-H",
    "X-GitHub-Api-Version: 2026-03-10",
    "--method",
    method,
  ];
  if (body !== undefined) argumentsList.push("--input", "-");
  argumentsList.push(endpoint);
  const output = command("gh", argumentsList, {
    input: body === undefined ? undefined : JSON.stringify(body),
  });
  return output ? (JSON.parse(output) as unknown) : null;
}

function sorted(values: string[]) {
  return [...values].sort((left, right) => left.localeCompare(right));
}

function assertExactNames(
  label: string,
  expected: string[],
  actual: NameMetadata[],
) {
  const expectedNames = sorted(expected);
  const actualNames = sorted(actual.map(({ name }) => name));
  if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
    throw new Error(`${label} names do not match the governed scope.`);
  }
}

function listConfiguration(
  type: "secret" | "variable",
  environment?: string,
) {
  const argumentsList = [
    type,
    "list",
    "--repo",
    repository,
    "--json",
    "name,updatedAt",
  ];
  if (environment) argumentsList.push("--env", environment);
  const output = command("gh", argumentsList);
  return JSON.parse(output ?? "[]") as NameMetadata[];
}

function environmentState(environment: string) {
  return ghApi(`repos/${repository}/environments/${environment}`) as {
    deployment_branch_policy?: {
      custom_branch_policies?: boolean;
      protected_branches?: boolean;
    };
    protection_rules?: Array<{
      prevent_self_review?: boolean;
      reviewers?: Array<{
        reviewer?: { login?: string };
        type?: string;
      }>;
      type?: string;
      wait_timer?: number;
    }>;
  };
}

function branchPolicies(environment: string) {
  return ghApi(
    `repos/${repository}/environments/${environment}/deployment-branch-policies`,
  ) as {
    branch_policies?: Array<{ id?: number; name?: string; type?: string }>;
  };
}

function verifyEnvironment(environment: "macos-release" | "public-release") {
  const expected = governance.environments[environment];
  const state = environmentState(environment);
  const reviewerRule = state.protection_rules?.find(
    (rule) => rule.type === "required_reviewers",
  );
  const reviewers =
    reviewerRule?.reviewers?.flatMap((reviewer) =>
      reviewer.type === "User" && reviewer.reviewer?.login
        ? [reviewer.reviewer.login]
        : [],
    ) ?? [];
  const waitTimer =
    state.protection_rules?.find((rule) => rule.type === "wait_timer")
      ?.wait_timer ?? 0;
  const policies = branchPolicies(environment).branch_policies ?? [];
  if (
    reviewerRule?.prevent_self_review !== expected.prevent_self_review ||
    JSON.stringify(sorted(reviewers)) !==
      JSON.stringify(sorted(expected.required_reviewers)) ||
    waitTimer !== expected.wait_timer_minutes ||
    state.deployment_branch_policy?.custom_branch_policies !== true ||
    state.deployment_branch_policy?.protected_branches !== false ||
    JSON.stringify(sorted(policies.map(({ name }) => name ?? ""))) !==
      JSON.stringify(sorted(expected.deployment_tag_patterns))
  ) {
    throw new Error(`${environment} protection does not match governance.`);
  }

  const secrets = listConfiguration("secret", environment);
  const variables = listConfiguration("variable", environment);
  assertExactNames(`${environment} secret`, expected.secrets, secrets);
  assertExactNames(`${environment} variable`, expected.variables, variables);
  return {
    environment,
    reviewer: reviewers[0],
    secret_names: secrets,
    tag_patterns: policies.map(({ name }) => name),
    variable_names: variables,
  };
}

function verifyActions() {
  const permissions = ghApi(`repos/${repository}/actions/permissions`) as {
    allowed_actions?: unknown;
    sha_pinning_required?: unknown;
  };
  const selected = ghApi(
    `repos/${repository}/actions/permissions/selected-actions`,
  ) as {
    github_owned_allowed?: unknown;
    patterns_allowed?: unknown;
    verified_allowed?: unknown;
  };
  const workflow = ghApi(
    `repos/${repository}/actions/permissions/workflow`,
  ) as {
    can_approve_pull_request_reviews?: unknown;
    default_workflow_permissions?: unknown;
  };
  if (
    permissions.allowed_actions !== governance.actions.allowed_actions ||
    permissions.sha_pinning_required !==
      governance.actions.sha_pinning_required ||
    selected.github_owned_allowed !==
      governance.actions.github_owned_allowed ||
    selected.verified_allowed !== governance.actions.verified_allowed ||
    JSON.stringify(sorted((selected.patterns_allowed as string[]) ?? [])) !==
      JSON.stringify(sorted(governance.actions.patterns_allowed)) ||
    workflow.default_workflow_permissions !== "read" ||
    workflow.can_approve_pull_request_reviews !== false
  ) {
    throw new Error("GitHub Actions permissions do not match governance.");
  }
}

function verifyTagRuleset() {
  const summaries = ghApi(`repos/${repository}/rulesets`) as Array<{
    id?: number;
    name?: string;
  }>;
  const summary = summaries.find(
    ({ name }) => name === "Immutable stable release tags",
  );
  if (!summary?.id) throw new Error("The immutable tag ruleset is absent.");
  const ruleset = ghApi(`repos/${repository}/rulesets/${summary.id}`) as {
    bypass_actors?: unknown[];
    conditions?: { ref_name?: { include?: string[] } };
    enforcement?: unknown;
    rules?: Array<{ type?: string }>;
    target?: unknown;
  };
  if (
    ruleset.target !== "tag" ||
    ruleset.enforcement !== governance.tag_ruleset.enforcement ||
    JSON.stringify(ruleset.bypass_actors ?? []) !== "[]" ||
    JSON.stringify(sorted(ruleset.conditions?.ref_name?.include ?? [])) !==
      JSON.stringify(sorted(governance.tag_ruleset.include)) ||
    JSON.stringify(sorted(ruleset.rules?.map(({ type }) => type ?? "") ?? [])) !==
      JSON.stringify(sorted(governance.tag_ruleset.rules))
  ) {
    throw new Error("The immutable tag ruleset does not match governance.");
  }
}

function verifyRepositoryScopes() {
  const protectedNames = new Set(
    Object.values(governance.environments).flatMap((environment) =>
      environment.secrets.concat(environment.variables),
    ),
  );
  const repositoryNames = [
    ...listConfiguration("secret"),
    ...listConfiguration("variable"),
  ].filter(({ name }) => protectedNames.has(name));
  if (repositoryNames.length > 0) {
    throw new Error("Release configuration has a repository-scoped copy.");
  }
}

function verifyImmutableReleases() {
  const state = ghApi(`repos/${repository}/immutable-releases`) as {
    enabled?: unknown;
  };
  if (state.enabled !== governance.immutable_releases) {
    throw new Error("Immutable Releases are not enabled.");
  }
}

function verifyPublicUpdaterConfiguration() {
  const config = JSON.parse(
    readFileSync(
      resolve(
        workspaceRoot,
        "apps",
        "desktop",
        "src-tauri",
        "tauri.conf.json",
      ),
      "utf8",
    ),
  ) as { plugins?: { updater?: { endpoints?: unknown; pubkey?: unknown } } };
  const updater = config.plugins?.updater;
  if (
    JSON.stringify(updater?.endpoints) !==
      JSON.stringify([stableUpdaterEndpoint]) ||
    typeof updater?.pubkey !== "string" ||
    updater.pubkey.length === 0 ||
    updater.pubkey.includes("NOT_CONFIGURED")
  ) {
    throw new Error("The public updater configuration is not ready.");
  }
}

function verify() {
  const environments = [
    verifyEnvironment("macos-release"),
    verifyEnvironment("public-release"),
  ];
  verifyActions();
  verifyTagRuleset();
  verifyRepositoryScopes();
  verifyImmutableReleases();
  verifyPublicUpdaterConfiguration();
  console.log(
    JSON.stringify(
      {
        automated_status: "PASS",
        environments,
        manual_checks: {
          administrator_bypass: "NOT_VERIFIED",
        },
        protected_values_read: false,
        protected_values_emitted: false,
      },
      null,
      2,
    ),
  );
  console.log(
    "Manual check: administrator bypass is disabled for both environments.",
  );
}

function assertRemoteMainIsExact() {
  if (command("git", ["status", "--porcelain"]) !== "") {
    throw new Error("Governance activation requires a clean worktree.");
  }
  command("git", ["fetch", "--no-tags", "origin", "main"]);
  const head = command("git", ["rev-parse", "HEAD"]);
  const remoteMain = command("git", ["rev-parse", "origin/main"]);
  const branch = command("git", ["branch", "--show-current"]);
  if (branch !== "main" || head !== remoteMain) {
    throw new Error("Governance activation requires exact remote main.");
  }
}

function applyEnvironment(
  environment: "macos-release" | "public-release",
  reviewerId: number,
) {
  const expected = governance.environments[environment];
  ghApi(`repos/${repository}/environments/${environment}`, "PUT", {
    deployment_branch_policy: {
      custom_branch_policies: true,
      protected_branches: false,
    },
    prevent_self_review: expected.prevent_self_review,
    reviewers: [{ id: reviewerId, type: "User" }],
    wait_timer: expected.wait_timer_minutes,
  });
  const policies = branchPolicies(environment).branch_policies ?? [];
  if (policies.length === 0) {
    ghApi(
      `repos/${repository}/environments/${environment}/deployment-branch-policies`,
      "POST",
      { name: expected.deployment_tag_patterns[0], type: "tag" },
    );
  } else if (
    JSON.stringify(sorted(policies.map(({ name }) => name ?? ""))) !==
    JSON.stringify(sorted(expected.deployment_tag_patterns))
  ) {
    throw new Error(`${environment} has unexpected deployment patterns.`);
  }
}

function applyTagRuleset() {
  const summaries = ghApi(`repos/${repository}/rulesets`) as Array<{
    id?: number;
    name?: string;
  }>;
  const current = summaries.find(
    ({ name }) => name === "Immutable stable release tags",
  );
  const endpoint = current?.id
    ? `repos/${repository}/rulesets/${current.id}`
    : `repos/${repository}/rulesets`;
  ghApi(endpoint, current?.id ? "PUT" : "POST", {
    bypass_actors: [],
    conditions: { ref_name: { exclude: [], include: governance.tag_ruleset.include } },
    enforcement: governance.tag_ruleset.enforcement,
    name: "Immutable stable release tags",
    rules: [
      { parameters: { update_allows_fetch_and_merge: false }, type: "update" },
      { type: "deletion" },
    ],
    target: "tag",
  });
}

function apply() {
  assertRemoteMainIsExact();
  const reviewer = ghApi(`users/${governance.reviewer}`) as { id?: unknown };
  if (typeof reviewer.id !== "number") {
    throw new Error("The governed reviewer cannot be resolved.");
  }
  applyEnvironment("macos-release", reviewer.id);
  applyEnvironment("public-release", reviewer.id);
  applyTagRuleset();
  ghApi(`repos/${repository}/immutable-releases`, "PUT");
  ghApi(`repos/${repository}/actions/permissions`, "PUT", {
    allowed_actions: governance.actions.allowed_actions,
    enabled: true,
    sha_pinning_required: governance.actions.sha_pinning_required,
  });
  ghApi(`repos/${repository}/actions/permissions/selected-actions`, "PUT", {
    github_owned_allowed: governance.actions.github_owned_allowed,
    patterns_allowed: governance.actions.patterns_allowed,
    verified_allowed: governance.actions.verified_allowed,
  });
  ghApi(`repos/${repository}/actions/permissions/workflow`, "PUT", {
    can_approve_pull_request_reviews: false,
    default_workflow_permissions: "read",
  });
  console.log("Repository release governance: applied.");
  console.log(
    "Do: disable administrator bypass for macos-release and public-release in GitHub Settings.",
  );
  console.log(
    "Do: add the exact governed environment secret and variable names, then run --verify.",
  );
}

const mode = process.argv[2];
if (process.argv.length !== 3 || !new Set(["--apply", "--verify"]).has(mode)) {
  throw new Error("Use --apply or --verify.");
}
if (mode === "--apply") apply();
else verify();
