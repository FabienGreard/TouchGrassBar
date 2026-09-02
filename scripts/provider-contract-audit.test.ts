import { describe, expect, test, vi } from "vitest";

import {
  auditProviderContracts,
  renderProviderAuditMarkdown,
  semanticCodexRustAnchorSha256,
  semanticJsonSha256,
  semanticPricingEvidenceSectionSha256,
  semanticPricingEvidenceWindowSha256,
  semanticPricingRuleWindowSha256,
  type ReviewedProviderContract,
} from "./provider-contract-audit";

const workspaceRoot = "/workspace";
const now = new Date("2026-08-15T12:00:00.000Z");
const codexSchema = {
  properties: {
    dailyUsageBuckets: { type: "array" },
  },
  type: "object",
};
const codexProtocolSource = `
pub enum EventMsg {
    Error(ErrorEvent),
    TokenCount(TokenCountEvent),
}
`;
const codexTokenMappingSource = `
impl From<ResponseCompletedUsage> for TokenUsage {
    fn from(val: ResponseCompletedUsage) -> Self {
        let input_tokens_details = val.input_tokens_details.unwrap_or_default();
        TokenUsage {
            input_tokens: val.input_tokens,
            cached_input_tokens: input_tokens_details.cached_tokens,
            cache_write_input_tokens: input_tokens_details.cache_write_tokens,
            output_tokens: val.output_tokens,
            reasoning_output_tokens: val.output_tokens_details
                .map(|d| d.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: val.total_tokens,
        }
    }
}

fn parses_cache_write_token_usage() {
    let usage = json!({
        "input_tokens": 100,
        "input_tokens_details": {
            "cached_tokens": 40,
            "cache_write_tokens": 60
        },
        "output_tokens": 10,
        "output_tokens_details": { "reasoning_tokens": 5 },
        "total_tokens": 110
    });
}
`;
const codexModels = {
  models: [
    {
      context_window: 1_000,
      slug: "gpt-test",
      supported_in_api: true,
      visibility: "list",
    },
  ],
};
const openAiManifest = {
  basis: "test-openai",
  models: [
    {
      aliases: [],
      name: "gpt-test",
      periods: [
        {
          cacheWriteUsdPerMillion: 2.5,
          cachedInputUsdPerMillion: 0.2,
          effectiveFrom: "2026-01-01",
          effectiveUntil: null,
          fastLongContext: {
            cacheWriteUsdPerMillion: 10,
            cachedInputUsdPerMillion: 0.8,
            effectiveFrom: "2026-01-01",
            effectiveUntil: null,
            inputUsdPerMillion: 8,
            outputUsdPerMillion: 30,
          },
          fastMultiplier: 2,
          inputUsdPerMillion: 2,
          longContext: {
            inputMultiplier: 2,
            inputTokensAbove: 1_000,
            outputMultiplier: 1.5,
          },
          outputUsdPerMillion: 10,
        },
      ],
    },
  ],
  schemaVersion: 1,
};
const anthropicManifest = {
  basis: "test-anthropic",
  batchFactor: 0.5,
  models: [
    {
      aliases: [],
      fastPeriods: [
        {
          cacheReadUsdPerMillion: 1,
          cacheWrite1hUsdPerMillion: 20,
          cacheWrite5mUsdPerMillion: 12.5,
          effectiveFrom: "2026-01-01",
          effectiveUntil: null,
          inputUsdPerMillion: 10,
          outputUsdPerMillion: 50,
        },
      ],
      name: "claude-sonnet-5",
      standardPeriods: [
        {
          cacheReadUsdPerMillion: 0.2,
          cacheWrite1hUsdPerMillion: 4,
          cacheWrite5mUsdPerMillion: 2.5,
          effectiveFrom: "2026-01-01",
          effectiveUntil: null,
          inputUsdPerMillion: 2,
          outputUsdPerMillion: 10,
        },
      ],
      supportsUsInference: true,
    },
  ],
  schemaVersion: 1,
  usInferenceFactor: 1.1,
  webSearchUsdPerThousand: 10,
};

const openAiPricing = `
# Pricing

### Standard pricing data

| Model | Input | Cached input | Cache write | Output | Long input | Long cached input | Long cache write | Long output |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-test | $2 / M tokens | $0.2 / M tokens | $2.5 / M tokens | $10 / M tokens | $4 / M tokens | $0.4 / M tokens | $5 / M tokens | $15 / M tokens |

Regional processing (data residency) endpoints are charged a 10% uplift for models released on or after March 5, 2026.
Priority processing was renamed Fast mode on July 30, 2026.

### Fast pricing data

| Model | Input | Cached input | Cache write | Output | Long input | Long cached input | Long cache write | Long output |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-test | $4 / M tokens | $0.4 / M tokens | $5 / M tokens | $20 / M tokens | $8 / M tokens | $0.8 / M tokens | $10 / M tokens | $30 / M tokens |

### Other pricing data
`;

const anthropicPricing = `
## Model pricing

| Model | Input | 5-minute cache write | 1-hour cache write | Cache read | Output |
| --- | --- | --- | --- | --- | --- |
| Claude Sonnet 5 | $2 / MTok | $2.5 / MTok | $4 / MTok | $0.2 / MTok | $10 / MTok |

### Fast mode pricing

| Model | Input | Output |
| --- | --- | --- |
| Claude Sonnet 5 | $10 / MTok | $50 / MTok |

Batch receives a 50% discount.
For Claude 4.6 and later models, US inference has a 1.1x pricing multiplier.
Web search costs $10 per 1,000 searches.

| Cache operation | Rate |
| --- | --- |
| 5-minute cache write | 1.25x base input price |
| 1-hour cache write | 2x base input price |
| Cache read (hit) | 0.1x base input price |

Web fetch usage has no additional charges.
Fast mode is not available with the Batch API.

## Other pricing
`;

const usageFields = {
  BetaCacheCreation: {
    ephemeral_1h_input_tokens: "required:number",
    ephemeral_5m_input_tokens: "required:number",
  },
  BetaServerToolUsage: {
    web_fetch_requests: "required:number",
    web_search_requests: "required:number",
  },
  BetaUsage: {
    cache_creation_input_tokens: "required:number | null",
    cache_read_input_tokens: "required:number | null",
    input_tokens: "required:number",
    output_tokens: "required:number",
    server_tool_use: "required:BetaServerToolUsage | null",
  },
};

const usageTypes = Object.entries(usageFields)
  .map(
    ([name, fields]) =>
      `export interface ${name} {\n${Object.entries(fields)
        .map(([field, signature]) => {
          const separator = signature.indexOf(":");
          const presence = signature.slice(0, separator);
          const fieldType = signature.slice(separator + 1);
          return `  ${field}${presence === "optional" ? "?" : ""}: ${fieldType};`;
        })
        .join("\n")}\n}`,
  )
  .join("\n\n");
const promptCachingSemantics = `
**\`input_tokens\` is the uncached remainder only.**
Total prompt size = \`input_tokens + cache_creation_input_tokens + cache_read_input_tokens\`.
`;
const openAiDatedPricingEvidence = `
## January, 2026

### Jan 1

GPT Test standard pricing started on January 1, 2026.
GPT Test Fast long-context pricing started on January 1, 2026.
`;
const anthropicDatedPricingEvidence = `
### August 10, 2026

The planned September 1, 2026 price change will not occur.

### January 1, 2026

Claude Sonnet 5 standard pricing started on January 1, 2026.
Claude Sonnet 5 Fast pricing started on January 1, 2026.
`;
const usageTypesWithSemantics = `${usageTypes}

/**
 * \`output_tokens\` remains the inclusive, authoritative total used for billing.
 */
`;

const reviewedContract: ReviewedProviderContract = {
  claude: {
    agentSdkPackageUrl: "https://registry.npmjs.org/@anthropic-ai%2fclaude-agent-sdk/latest",
    latestPackageUrl: "https://registry.npmjs.org/@anthropic-ai%2fclaude-code/latest",
    parserSourcePath: "fixtures/claude-usage.rs",
    pricingManifestPath: "fixtures/anthropic-standard.json",
    pricingManifestSemanticSha256: semanticJsonSha256(anthropicManifest),
    pricingRuleWindows: [
      {
        endHeading: "## Other pricing",
        id: "claude-token-and-feature-pricing",
        semanticSha256: semanticPricingRuleWindowSha256(
          anthropicPricing,
          "## Model pricing",
          "## Other pricing",
        ),
        startHeading: "## Model pricing",
      },
    ],
    pricingEvidence: {
      boundaryExemptions: [],
      sources: [
        {
          id: "release-notes",
          sections: [
            {
              checkpoints: [
                {
                  boundary: "absent",
                  date: "2026-09-01",
                  marker: "The planned September 1, 2026 price change will not occur.",
                  model: "claude-sonnet-5",
                  periodKind: "standard",
                },
              ],
              date: "2026-08-10",
              heading: "### August 10, 2026",
              id: "canceled-price-change",
              selector: "The planned September 1, 2026 price change will not occur.",
              semanticSha256: semanticPricingEvidenceSectionSha256(
                anthropicDatedPricingEvidence,
                "### August 10, 2026",
                "The planned September 1, 2026 price change will not occur.",
              ),
            },
            {
              checkpoints: [
                {
                  boundary: "start",
                  model: "claude-sonnet-5",
                  periodKind: "standard",
                },
                {
                  boundary: "start",
                  model: "claude-sonnet-5",
                  periodKind: "fast",
                },
              ],
              date: "2026-01-01",
              heading: "### January 1, 2026",
              id: "sonnet-start",
              selector: "Claude Sonnet 5 standard pricing started",
              semanticSha256: semanticPricingEvidenceSectionSha256(
                anthropicDatedPricingEvidence,
                "### January 1, 2026",
                "Claude Sonnet 5 standard pricing started",
              ),
            },
          ],
          url: "https://platform.claude.com/docs/en/release-notes/overview.md",
          windowSemanticSha256: semanticPricingEvidenceWindowSha256(anthropicDatedPricingEvidence, [
            {
              heading: "### August 10, 2026",
              selector: "The planned September 1, 2026 price change will not occur.",
            },
            {
              heading: "### January 1, 2026",
              selector: "Claude Sonnet 5 standard pricing started",
            },
          ]),
        },
      ],
    },
    pricingSourceUrl: "https://platform.claude.com/docs/en/about-claude/pricing.md",
    reviewedAgentSdkVersion: "0.3.224",
    reviewedLatestVersion: "2.1.224",
    reviewedStableVersion: "2.1.224",
    stablePackageUrl: "https://registry.npmjs.org/@anthropic-ai%2fclaude-code/stable",
    usageInterfaces: usageFields,
    usageSemanticSources: [
      {
        id: "api-output-inclusive",
        markers: ["output_tokens remains the inclusive, authoritative total used for billing."],
        url: "https://raw.githubusercontent.com/anthropics/anthropic-sdk-typescript/main/messages.ts",
      },
      {
        id: "prompt-cache-input-sum",
        markers: [
          "input_tokens is the uncached remainder only.",
          "Total prompt size = input_tokens + cache_creation_input_tokens + cache_read_input_tokens.",
        ],
        url: "https://raw.githubusercontent.com/anthropics/skills/main/prompt-caching.md",
      },
    ],
    usageTypesSourceUrl:
      "https://raw.githubusercontent.com/anthropics/anthropic-sdk-typescript/main/messages.ts",
  },
  codex: {
    modelCatalogPath: "models.json",
    parserSourcePath: "fixtures/codex-usage.rs",
    pricingManifestPath: "fixtures/openai-standard.json",
    pricingManifestSemanticSha256: semanticJsonSha256(openAiManifest),
    pricingRuleWindows: [
      {
        endHeading: "### Other pricing data",
        id: "openai-token-pricing",
        semanticSha256: semanticPricingRuleWindowSha256(
          openAiPricing,
          "# Pricing",
          "### Other pricing data",
        ),
        startHeading: "# Pricing",
      },
    ],
    pricingEvidence: {
      boundaryExemptions: [],
      sources: [
        {
          id: "api-changelog",
          sections: [
            {
              checkpoints: [
                { boundary: "start", model: "gpt-test", periodKind: "standard" },
                {
                  boundary: "start",
                  model: "gpt-test",
                  periodKind: "fast-long-context",
                },
              ],
              date: "2026-01-01",
              heading: "### Jan 1",
              id: "gpt-test-start",
              selector: "GPT Test standard pricing started",
              semanticSha256: semanticPricingEvidenceSectionSha256(
                openAiDatedPricingEvidence,
                "### Jan 1",
                "GPT Test standard pricing started",
              ),
            },
          ],
          url: "https://developers.openai.com/api/docs/changelog.md",
          windowSemanticSha256: semanticPricingEvidenceWindowSha256(openAiDatedPricingEvidence, [
            { heading: "### Jan 1", selector: "GPT Test standard pricing started" },
          ]),
        },
      ],
    },
    pricingSemanticMarkers: [
      "Regional processing (data residency) endpoints are charged a 10% uplift for models released on or after March 5, 2026",
      "Priority processing was renamed Fast mode on July 30, 2026.",
    ],
    pricingSourceUrl: "https://developers.openai.com/api/docs/pricing.md",
    releaseUrl: "https://api.github.com/repos/openai/codex/releases/latest",
    repositoryRawBaseUrl: "https://raw.githubusercontent.com/openai/codex",
    reviewedRelease: "0.147.0",
    rustAnchors: [
      {
        id: "rollout-event-kinds",
        kind: "event-msg-variants",
        path: "protocol.rs",
        semanticSha256: semanticCodexRustAnchorSha256(codexProtocolSource, "event-msg-variants"),
      },
      {
        id: "api-token-mapping",
        kind: "response-token-mapping",
        path: "responses.rs",
        semanticSha256: semanticCodexRustAnchorSha256(
          codexTokenMappingSource,
          "response-token-mapping",
        ),
      },
    ],
    schemas: [
      {
        id: "account-token-usage",
        path: "schemas/account-token-usage.json",
        semanticSha256: semanticJsonSha256(codexSchema),
      },
    ],
  },
  reviewEveryDays: 30,
  reviewedAt: "2026-08-01",
  schemaVersion: 1,
};

type RemoteSource =
  | Error
  | string
  | {
      body: string;
      headers?: HeadersInit;
      status?: number;
    };

const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
const json = (value: unknown) => JSON.stringify(value);

function createScenario() {
  const contract = clone(reviewedContract);
  const codexRawUrl = (tag: string, path: string) =>
    `${contract.codex.repositoryRawBaseUrl}/${tag}/${path}`;
  const localSources = new Map<string, string>([
    [
      `${workspaceRoot}/${contract.codex.parserSourcePath}`,
      "const MIN_SUPPORTED_CODEX_CLI_MINOR: u16 = 147;\nconst MAX_SUPPORTED_CODEX_CLI_MINOR: u16 = 147;",
    ],
    [`${workspaceRoot}/${contract.codex.pricingManifestPath}`, json(openAiManifest)],
    [
      `${workspaceRoot}/${contract.claude.parserSourcePath}`,
      'const SUPPORTED_CLAUDE_CODE_VERSIONS: [&str; 1] = ["2.1.224"];',
    ],
    [`${workspaceRoot}/${contract.claude.pricingManifestPath}`, json(anthropicManifest)],
  ]);
  const remoteSources = new Map<string, RemoteSource>([
    [contract.codex.releaseUrl, json({ tag_name: "rust-v0.147.0" })],
    [codexRawUrl("rust-v0.147.0", contract.codex.schemas[0]?.path ?? ""), json(codexSchema)],
    [codexRawUrl("rust-v0.147.0", contract.codex.modelCatalogPath), json(codexModels)],
    [codexRawUrl("rust-v0.147.0", contract.codex.rustAnchors[0]?.path ?? ""), codexProtocolSource],
    [
      codexRawUrl("rust-v0.147.0", contract.codex.rustAnchors[1]?.path ?? ""),
      codexTokenMappingSource,
    ],
    [contract.codex.pricingSourceUrl, openAiPricing],
    [contract.codex.pricingEvidence.sources[0]?.url ?? "", openAiDatedPricingEvidence],
    [contract.claude.stablePackageUrl, json({ version: "2.1.224" })],
    [contract.claude.latestPackageUrl, json({ version: "2.1.224" })],
    [contract.claude.agentSdkPackageUrl, json({ version: "0.3.224" })],
    [contract.claude.usageTypesSourceUrl, usageTypesWithSemantics],
    [contract.claude.usageSemanticSources[1]?.url ?? "", promptCachingSemantics],
    [contract.claude.pricingSourceUrl, anthropicPricing],
    [contract.claude.pricingEvidence.sources[0]?.url ?? "", anthropicDatedPricingEvidence],
  ]);
  const readText = vi.fn((path: string) => {
    const source = localSources.get(path);
    if (source === undefined) throw new Error("Mock local source is absent.");
    return source;
  });
  const fetcher = vi.fn(async (input: string | URL | Request, _init?: RequestInit) => {
    const url = input instanceof Request ? input.url : input.toString();
    const source = remoteSources.get(url);
    if (source === undefined) throw new Error("Mock remote source is absent.");
    if (source instanceof Error) throw source;
    const response = typeof source === "string" ? { body: source } : source;
    return new Response(response.body, {
      headers: response.headers,
      status: response.status ?? 200,
    });
  });
  const audit = () =>
    auditProviderContracts({
      contract,
      fetcher,
      now,
      readText,
      workspaceRoot,
    });

  return {
    audit,
    codexRawUrl,
    contract,
    fetcher,
    localSources,
    readText,
    remoteSources,
  };
}

describe("provider contract audit", () => {
  test("passes when every normalized source matches the reviewed contract", async () => {
    const scenario = createScenario();

    const report = await scenario.audit();

    expect(report).toMatchObject({
      checkedAt: now.toISOString(),
      findings: [],
      sourceCount: 14,
      status: "pass",
    });
    expect(scenario.fetcher).toHaveBeenCalledTimes(14);
    expect(scenario.readText).toHaveBeenCalledTimes(4);
  });

  test("never sends a GitHub credential with source requests", async () => {
    vi.stubEnv("GITHUB_TOKEN", "PRIVATE-GITHUB-TOKEN");
    try {
      const scenario = createScenario();

      await scenario.audit();

      for (const [, init] of scenario.fetcher.mock.calls) {
        expect(new Headers(init?.headers).has("authorization")).toBe(false);
      }
    } finally {
      vi.unstubAllEnvs();
    }
  });

  test("never reports pass when a first-party source is unavailable", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.contract.claude.latestPackageUrl,
      json({ version: "2.1.225" }),
    );
    scenario.remoteSources.set(
      scenario.contract.claude.usageTypesSourceUrl,
      new Error("simulated timeout"),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("unavailable");
    expect(report.status).not.toBe("pass");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "upstream-release-changed",
        provider: "claude",
        status: "review-required",
      }),
    );
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "source-unavailable",
        provider: "claude",
        status: "unavailable",
      }),
    );
  });

  test("never reports pass for valid JSON with an invalid required shape", async () => {
    const cases: Array<{
      change: (scenario: ReturnType<typeof createScenario>) => void;
      code?: string;
      name: string;
    }> = [
      {
        name: "release",
        change: (scenario) => {
          scenario.remoteSources.set(scenario.contract.codex.releaseUrl, "[]");
        },
      },
      {
        name: "generated schema",
        change: (scenario) => {
          scenario.remoteSources.set(
            scenario.codexRawUrl("rust-v0.147.0", scenario.contract.codex.schemas[0]?.path ?? ""),
            "null",
          );
        },
      },
      {
        name: "model catalog",
        change: (scenario) => {
          scenario.remoteSources.set(
            scenario.codexRawUrl("rust-v0.147.0", scenario.contract.codex.modelCatalogPath),
            "[]",
          );
        },
      },
      {
        name: "Agent SDK package",
        change: (scenario) => {
          scenario.remoteSources.set(scenario.contract.claude.agentSdkPackageUrl, "null");
        },
      },
      {
        name: "local pricing manifest",
        code: "invalid-local-contract",
        change: (scenario) => {
          scenario.localSources.set(
            `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
            "[]",
          );
        },
      },
    ];

    for (const entry of cases) {
      const scenario = createScenario();
      entry.change(scenario);

      const report = await scenario.audit();

      expect(report.status, entry.name).toBe("unavailable");
      expect(report.findings, entry.name).toContainEqual(
        expect.objectContaining({
          code: entry.code ?? "invalid-source",
          status: "unavailable",
        }),
      );
    }
  });

  test("rejects a reviewed contract that disables required checks", async () => {
    const cases: Array<(scenario: ReturnType<typeof createScenario>) => void> = [
      (scenario) => {
        scenario.contract.reviewedAt = "2026-02-31";
      },
      (scenario) => {
        scenario.contract.codex.schemas = [];
      },
      (scenario) => {
        scenario.contract.codex.rustAnchors = [];
      },
      (scenario) => {
        scenario.contract.codex.pricingSemanticMarkers = [];
      },
      (scenario) => {
        scenario.contract.codex.pricingSemanticMarkers[0] = "   ";
      },
      (scenario) => {
        scenario.contract.codex.pricingEvidence.sources = [];
      },
      (scenario) => {
        scenario.contract.codex.pricingRuleWindows = [];
      },
      (scenario) => {
        scenario.contract.claude.usageInterfaces = {};
      },
      (scenario) => {
        scenario.contract.claude.usageSemanticSources = [];
      },
      (scenario) => {
        scenario.contract.claude.pricingEvidence.sources = [];
      },
      (scenario) => {
        scenario.contract.claude.pricingRuleWindows = [];
      },
      (scenario) => {
        const checkpoint =
          scenario.contract.claude.pricingEvidence.sources[0]?.sections[0]?.checkpoints[0];
        if (checkpoint?.boundary === "absent") checkpoint.marker = "   ";
      },
    ];

    for (const change of cases) {
      const scenario = createScenario();
      change(scenario);

      const report = await scenario.audit();

      expect(report).toMatchObject({
        sourceCount: 0,
        status: "unavailable",
      });
      expect(report.findings).toEqual(
        expect.arrayContaining([
          expect.objectContaining({
            code: "invalid-local-contract",
            provider: "claude",
          }),
          expect.objectContaining({
            code: "invalid-local-contract",
            provider: "codex",
          }),
        ]),
      );
    }
  });

  test("requires review for a new release and changed generated schema", async () => {
    const scenario = createScenario();
    const releaseTag = "rust-v0.148.0";
    scenario.remoteSources.set(scenario.contract.codex.releaseUrl, json({ tag_name: releaseTag }));
    scenario.remoteSources.set(
      scenario.codexRawUrl(releaseTag, scenario.contract.codex.schemas[0]?.path ?? ""),
      json({
        ...codexSchema,
        properties: {
          ...codexSchema.properties,
          threadUsage: { type: ["object", "null"] },
        },
      }),
    );
    scenario.remoteSources.set(
      scenario.codexRawUrl(releaseTag, scenario.contract.codex.modelCatalogPath),
      json(codexModels),
    );
    for (const anchor of scenario.contract.codex.rustAnchors) {
      scenario.remoteSources.set(
        scenario.codexRawUrl(releaseTag, anchor.path),
        anchor.kind === "event-msg-variants" ? codexProtocolSource : codexTokenMappingSource,
      );
    }

    const report = await scenario.audit();
    const codes = report.findings.map((entry) => entry.code);

    expect(report.status).toBe("review-required");
    expect(codes).toEqual(
      expect.arrayContaining(["schema-changed", "unsupported-version", "upstream-release-changed"]),
    );
    expect(report.findings).not.toContainEqual(expect.objectContaining({ status: "unavailable" }));
  });

  test("detects Codex rollout event and token-meaning changes", async () => {
    const scenario = createScenario();
    const eventAnchor = scenario.contract.codex.rustAnchors.find(
      (anchor) => anchor.kind === "event-msg-variants",
    );
    const mappingAnchor = scenario.contract.codex.rustAnchors.find(
      (anchor) => anchor.kind === "response-token-mapping",
    );
    if (!eventAnchor || !mappingAnchor) {
      throw new Error("The Codex test anchors are absent.");
    }
    scenario.remoteSources.set(
      scenario.codexRawUrl("rust-v0.147.0", eventAnchor.path),
      codexProtocolSource.replace(
        "    TokenCount(TokenCountEvent),",
        "    TokenCount(TokenCountEvent),\n    NewTokenTotal(NewTokenTotalEvent),",
      ),
    );
    scenario.remoteSources.set(
      scenario.codexRawUrl("rust-v0.147.0", mappingAnchor.path),
      codexTokenMappingSource.replace(
        "total_tokens: val.total_tokens",
        "total_tokens: val.input_tokens",
      ),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          code: "event-kind-changed",
          provider: "codex",
        }),
        expect.objectContaining({
          code: "token-semantics-changed",
          provider: "codex",
        }),
      ]),
    );
  });

  test("detects a schema documentation meaning change", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.codexRawUrl("rust-v0.147.0", scenario.contract.codex.schemas[0]?.path ?? ""),
      json({
        ...codexSchema,
        properties: {
          dailyUsageBuckets: {
            description: "This field now excludes one token category.",
            type: "array",
          },
        },
      }),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "schema-changed",
        provider: "codex",
      }),
    );
  });

  test("requires review when a Claude usage field type changes", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.contract.claude.usageTypesSourceUrl,
      usageTypes.replace("  input_tokens: number;", "  input_tokens: string;"),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "unknown-field",
        provider: "claude",
        summary: expect.stringContaining("~input_tokens"),
      }),
    );
  });

  test("detects Claude inclusive-total and cache-sum meaning changes", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.contract.claude.usageTypesSourceUrl,
      usageTypesWithSemantics.replace("inclusive, authoritative total", "additional detail"),
    );
    const promptSource = scenario.contract.claude.usageSemanticSources.find(
      (source) => source.id === "prompt-cache-input-sum",
    );
    if (!promptSource) throw new Error("The Claude semantic source is absent.");
    scenario.remoteSources.set(
      promptSource.url,
      promptCachingSemantics.replace(
        "cache_creation_input_tokens + cache_read_input_tokens",
        "cache_creation_input_tokens",
      ),
    );

    const report = await scenario.audit();
    const semantics = report.findings.filter((entry) => entry.code === "token-semantics-changed");

    expect(report.status).toBe("review-required");
    expect(semantics).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          provider: "claude",
          summary: expect.stringContaining("api-output-inclusive"),
        }),
        expect.objectContaining({
          provider: "claude",
          summary: expect.stringContaining("prompt-cache-input-sum"),
        }),
      ]),
    );
  });

  test("requires review for a visible model without a bundled rule", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.codexRawUrl("rust-v0.147.0", scenario.contract.codex.modelCatalogPath),
      json({
        models: [
          ...codexModels.models,
          {
            context_window: 2_000,
            slug: "gpt-new",
            supported_in_api: true,
            visibility: "list",
          },
        ],
      }),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "unknown-model",
        provider: "codex",
        status: "review-required",
        summary: expect.stringContaining("gpt-new"),
      }),
    );
  });

  test("detects an unreviewed effective-date change", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    if (!period) throw new Error("The test price period is absent.");
    period.effectiveFrom = "2026-02-01";
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "effective-date-changed",
        provider: "codex",
      }),
    );
    expect(report.findings).not.toContainEqual(
      expect.objectContaining({ code: "reviewed-snapshot-changed" }),
    );
  });

  test("detects a coordinated local date change against the old provider section", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    const section = scenario.contract.codex.pricingEvidence.sources[0]?.sections[0];
    if (!period || !section) throw new Error("The OpenAI date checkpoint is absent.");
    period.effectiveFrom = "2026-02-01";
    section.date = "2026-02-01";
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "pricing-evidence-changed",
        provider: "codex",
      }),
    );
    expect(report.findings).not.toContainEqual(
      expect.objectContaining({ code: "reviewed-snapshot-changed" }),
    );
  });

  test("requires exact evidence coverage for every manifest period", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    if (!period) throw new Error("The OpenAI test period is absent.");
    manifest.models.push({
      aliases: [],
      name: "gpt-unlisted-period",
      periods: [{ ...period }],
    });
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "effective-date-changed",
        provider: "codex",
        summary: expect.stringContaining("lack evidence"),
      }),
    );
  });

  test("detects an appended pricing correction that retains the old section", async () => {
    const scenario = createScenario();
    const evidenceUrl = scenario.contract.claude.pricingEvidence.sources[0]?.url;
    if (!evidenceUrl) throw new Error("The Claude pricing evidence source is absent.");
    scenario.remoteSources.set(
      evidenceUrl,
      `${anthropicDatedPricingEvidence}

### August 20, 2026

The September price change will now occur.`,
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "pricing-evidence-changed",
        provider: "claude",
      }),
    );
  });

  test("rejects an absent checkpoint for an unknown model", async () => {
    const scenario = createScenario();
    const checkpoint =
      scenario.contract.claude.pricingEvidence.sources[0]?.sections[0]?.checkpoints[0];
    if (checkpoint?.boundary !== "absent") {
      throw new Error("The absent Claude checkpoint is missing.");
    }
    checkpoint.model = "claude-misspelled-model";

    const report = await scenario.audit();

    expect(report.status).toBe("unavailable");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "invalid-local-contract",
        provider: "claude",
      }),
    );
  });

  test("detects a canceled future price before its effective date", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const model = manifest.models[0];
    if (!model) throw new Error("The test model is absent.");
    const currentPeriod = model.standardPeriods[0];
    if (!currentPeriod) throw new Error("The test price period is absent.");
    currentPeriod.effectiveUntil = "2026-09-01";
    model.standardPeriods.push({
      ...currentPeriod,
      cacheReadUsdPerMillion: 0.3,
      cacheWrite1hUsdPerMillion: 6,
      cacheWrite5mUsdPerMillion: 3.75,
      effectiveFrom: "2026-09-01",
      effectiveUntil: null,
      inputUsdPerMillion: 3,
      outputUsdPerMillion: 15,
    });
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );

    const report = await scenario.audit();

    expect(now.toISOString()).toContain("2026-08-15");
    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "future-price-changed",
        provider: "claude",
        status: "review-required",
        summary: expect.stringContaining("2026-09-01"),
      }),
    );
  });

  test("requires review when a current Claude model has no active price", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const period = manifest.models[0]?.standardPeriods[0];
    if (!period) throw new Error("The test price period is absent.");
    period.effectiveUntil = "2026-08-01";
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "unknown-price",
        provider: "claude",
        summary: expect.stringContaining("no current price period"),
      }),
    );
  });

  test("detects added pricing rules that retain every reviewed marker", async () => {
    const cases = [
      {
        boundary: "### Other pricing data",
        provider: "codex" as const,
        source: openAiPricing,
      },
      {
        boundary: "## Other pricing",
        provider: "claude" as const,
        source: anthropicPricing,
      },
    ];

    for (const entry of cases) {
      const scenario = createScenario();
      const contract = scenario.contract[entry.provider];
      scenario.remoteSources.set(
        contract.pricingSourceUrl,
        entry.source.replace(
          entry.boundary,
          `A new provider surcharge applies to these requests.\n\n${entry.boundary}`,
        ),
      );

      const report = await scenario.audit();

      expect(report.status, entry.provider).toBe("review-required");
      expect(report.findings, entry.provider).toContainEqual(
        expect.objectContaining({
          code: "pricing-modifier-changed",
          provider: entry.provider,
          summary: expect.stringContaining("rule window changed"),
        }),
      );
    }
  });

  test("detects changed Fast and US inference model support", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const model = manifest.models[0];
    if (!model) throw new Error("The test model is absent.");
    model.fastPeriods = [];
    model.supportsUsInference = false;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );

    const report = await scenario.audit();
    const summaries = report.findings
      .filter((entry) => entry.code === "pricing-modifier-changed")
      .map((entry) => entry.summary);

    expect(report.status).toBe("review-required");
    expect(summaries).toEqual(
      expect.arrayContaining([
        expect.stringContaining("Fast support"),
        expect.stringContaining("US inference support"),
      ]),
    );
  });

  test("detects a cache read multiplier that the pricing page does not document", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const period = manifest.models[0]?.standardPeriods[0] as
      | { cacheReadMultiplier?: number; cacheReadUsdPerMillion: number }
      | undefined;
    if (!period) throw new Error("The test price period is absent.");
    period.cacheReadMultiplier = 0.025;
    period.cacheReadUsdPerMillion = 0.05;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );

    const report = await scenario.audit();
    const summaries = report.findings
      .filter((entry) => entry.provider === "claude" && entry.code === "pricing-modifier-changed")
      .map((entry) => entry.summary);

    expect(report.status).toBe("review-required");
    expect(summaries).toEqual(
      expect.arrayContaining([expect.stringContaining("0.025x cache read exception")]),
    );
  });

  test("accepts a declared cache read multiplier that the pricing page documents", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const period = manifest.models[0]?.standardPeriods[0] as
      | { cacheReadMultiplier?: number; cacheReadUsdPerMillion: number }
      | undefined;
    if (!period) throw new Error("The test price period is absent.");
    period.cacheReadMultiplier = 0.025;
    period.cacheReadUsdPerMillion = 0.05;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.claude.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);
    const documentedPricing = anthropicPricing
      .replace(
        "| Cache read (hit) | 0.1x base input price |",
        "| Cache read (hit) | 0.1x base input price (0.025x on Claude Sonnet 5) |",
      )
      .replace("| $0.2 / MTok |", "| $0.05 / MTok |");
    const ruleWindow = scenario.contract.claude.pricingRuleWindows[0];
    if (!ruleWindow) throw new Error("The Claude test rule window is absent.");
    ruleWindow.semanticSha256 = semanticPricingRuleWindowSha256(
      documentedPricing,
      ruleWindow.startHeading,
      ruleWindow.endHeading,
    );
    scenario.remoteSources.set(scenario.contract.claude.pricingSourceUrl, documentedPricing);

    const report = await scenario.audit();
    const summaries = report.findings
      .filter((entry) => entry.provider === "claude" && entry.area === "pricing")
      .map((entry) => entry.summary);

    expect(summaries).toEqual([]);
  });

  test("detects a cache read price that contradicts its declared multiplier", async () => {
    const scenario = createScenario();
    const manifest = clone(anthropicManifest);
    const period = manifest.models[0]?.standardPeriods[0];
    if (!period) throw new Error("The test price period is absent.");
    period.cacheReadUsdPerMillion = 0.5;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.claude.pricingManifestPath}`,
      json(manifest),
    );

    const report = await scenario.audit();
    const summaries = report.findings
      .filter((entry) => entry.provider === "claude" && entry.code === "pricing-modifier-changed")
      .map((entry) => entry.summary);

    expect(report.status).toBe("review-required");
    expect(summaries).toEqual(
      expect.arrayContaining([expect.stringContaining("does not match its declared multiplier")]),
    );
  });

  test("detects changed OpenAI Fast rates and pricing modifiers", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.contract.codex.pricingSourceUrl,
      openAiPricing
        .replace(
          "$4 / M tokens | $0.4 / M tokens | $5 / M tokens | $20 / M tokens",
          "$5 / M tokens | $0.5 / M tokens | $6 / M tokens | $25 / M tokens",
        )
        .replace(
          "Priority processing was renamed Fast mode on July 30, 2026.",
          "Priority processing uses a new service tier.",
        ),
    );

    const report = await scenario.audit();
    const findings = report.findings.filter((entry) => entry.code === "pricing-modifier-changed");

    expect(report.status).toBe("review-required");
    expect(findings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          provider: "codex",
          summary: expect.stringContaining("semantic marker"),
        }),
        expect.objectContaining({
          provider: "codex",
          summary: expect.stringContaining("official Fast pricing differs"),
        }),
      ]),
    );
  });

  test("accepts one unqualified OpenAI price row as an all-context rate", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    const ruleWindow = scenario.contract.codex.pricingRuleWindows[0];
    if (!period || !ruleWindow) throw new Error("The OpenAI test price period is absent.");
    period.longContext.inputMultiplier = 1;
    period.longContext.outputMultiplier = 1;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);
    const allContextPricing = openAiPricing.replace(
      "$4 / M tokens | $0.4 / M tokens | $5 / M tokens | $15 / M tokens",
      "- | - | - | -",
    );
    scenario.remoteSources.set(scenario.contract.codex.pricingSourceUrl, allContextPricing);
    ruleWindow.semanticSha256 = semanticPricingRuleWindowSha256(
      allContextPricing,
      ruleWindow.startHeading,
      ruleWindow.endHeading,
    );

    const report = await scenario.audit();

    expect(report.status).toBe("pass");
    expect(report.findings).not.toContainEqual(
      expect.objectContaining({
        code: "price-changed",
        provider: "codex",
      }),
    );
  });

  test("keeps a context-qualified OpenAI price row fail closed", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    const ruleWindow = scenario.contract.codex.pricingRuleWindows[0];
    if (!period || !ruleWindow) throw new Error("The OpenAI test price period is absent.");
    period.longContext.inputMultiplier = 1;
    period.longContext.outputMultiplier = 1;
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);
    const qualifiedPricing = openAiPricing
      .replace("| gpt-test | $2", "| gpt-test (<272K) | $2")
      .replace("$4 / M tokens | $0.4 / M tokens | $5 / M tokens | $15 / M tokens", "- | - | - | -");
    scenario.remoteSources.set(scenario.contract.codex.pricingSourceUrl, qualifiedPricing);
    ruleWindow.semanticSha256 = semanticPricingRuleWindowSha256(
      qualifiedPricing,
      ruleWindow.startHeading,
      ruleWindow.endHeading,
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "price-changed",
        provider: "codex",
      }),
    );
  });

  test("keeps a context-qualified OpenAI Fast row fail closed", async () => {
    const scenario = createScenario();
    const manifest = clone(openAiManifest);
    const period = manifest.models[0]?.periods[0];
    const ruleWindow = scenario.contract.codex.pricingRuleWindows[0];
    if (!period || !ruleWindow) throw new Error("The OpenAI test price period is absent.");
    delete period.fastLongContext;
    period.longContext.inputMultiplier = 1;
    period.longContext.outputMultiplier = 1;
    for (const source of scenario.contract.codex.pricingEvidence.sources) {
      for (const section of source.sections) {
        section.checkpoints = section.checkpoints.filter(
          (checkpoint) => checkpoint.periodKind !== "fast-long-context",
        );
      }
    }
    scenario.localSources.set(
      `${workspaceRoot}/${scenario.contract.codex.pricingManifestPath}`,
      json(manifest),
    );
    scenario.contract.codex.pricingManifestSemanticSha256 = semanticJsonSha256(manifest);
    const qualifiedFastPricing = openAiPricing
      .replace("$4 / M tokens | $0.4 / M tokens | $5 / M tokens | $15 / M tokens", "- | - | - | -")
      .replace("| gpt-test | $4 / M tokens", "| gpt-test (<272K) | $4 / M tokens")
      .replace(
        "$8 / M tokens | $0.8 / M tokens | $10 / M tokens | $30 / M tokens",
        "- | - | - | -",
      );
    scenario.remoteSources.set(scenario.contract.codex.pricingSourceUrl, qualifiedFastPricing);
    ruleWindow.semanticSha256 = semanticPricingRuleWindowSha256(
      qualifiedFastPricing,
      ruleWindow.startHeading,
      ruleWindow.endHeading,
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "pricing-modifier-changed",
        provider: "codex",
      }),
    );
  });

  test("keeps an unqualified OpenAI row fail closed when long-context rates differ", async () => {
    const scenario = createScenario();
    const ruleWindow = scenario.contract.codex.pricingRuleWindows[0];
    if (!ruleWindow) throw new Error("The OpenAI pricing rule window is absent.");
    const incompletePricing = openAiPricing.replace(
      "$4 / M tokens | $0.4 / M tokens | $5 / M tokens | $15 / M tokens",
      "- | - | - | -",
    );
    scenario.remoteSources.set(scenario.contract.codex.pricingSourceUrl, incompletePricing);
    ruleWindow.semanticSha256 = semanticPricingRuleWindowSha256(
      incompletePricing,
      ruleWindow.startHeading,
      ruleWindow.endHeading,
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "price-changed",
        provider: "codex",
      }),
    );
  });

  test("detects changed US support boundaries and Fast compatibility", async () => {
    const scenario = createScenario();
    scenario.remoteSources.set(
      scenario.contract.claude.pricingSourceUrl,
      anthropicPricing
        .replace("Claude 4.6 and later", "Claude 5 and later")
        .replace(
          "Fast mode is not available with the Batch API.",
          "Fast mode is available with the Batch API.",
        ),
    );

    const report = await scenario.audit();

    expect(report.status).toBe("review-required");
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "pricing-modifier-changed",
        provider: "claude",
      }),
    );
  });

  test("keeps oversized unsafe source content out of bounded Markdown", async () => {
    const scenario = createScenario();
    const unsafeBody =
      "prompt=private-prompt response=private-response Authorization=Bearer_sk-ant-secret /Users/alice/.claude/session.jsonl";
    scenario.remoteSources.set(scenario.contract.claude.usageTypesSourceUrl, {
      body: `${"x".repeat(2 * 1024 * 1024 + 1)}${unsafeBody}`,
    });

    const report = await scenario.audit();
    const markdown = renderProviderAuditMarkdown(report);

    expect(report.status).toBe("unavailable");
    expect(markdown.length).toBeLessThan(2_000);
    expect(markdown).toContain("authoritative public source is unavailable or unsafe");
    expect(markdown).not.toContain("private-prompt");
    expect(markdown).not.toContain("private-response");
    expect(markdown).not.toContain("sk-ant-secret");
    expect(markdown).not.toContain("/Users/alice");
    expect(markdown).not.toContain(workspaceRoot);
  });

  test("neutralizes GitHub mentions in rendered findings", () => {
    const markdown = renderProviderAuditMarkdown({
      checkedAt: "2026-08-26T00:00:00.000Z",
      findings: [
        {
          area: "pricing",
          code: "unknown-model",
          provider: "codex",
          status: "review-required",
          summary: "The remote model names @octocat and @example/team.",
        },
      ],
      reviewedAt: "2026-08-26",
      schemaVersion: 1,
      sourceCount: 1,
      status: "review-required",
    });

    expect(markdown).not.toContain("@octocat");
    expect(markdown).not.toContain("@example/team");
    expect(markdown).toContain("@\u200boctocat");
    expect(markdown).toContain("@\u200bexample/team");
  });

  test("bounds model drift and preserves a concurrent source failure", async () => {
    const scenario = createScenario();
    const models = [
      ...codexModels.models,
      ...Array.from({ length: 999 }, (_, index) => ({
        context_window: 1_000,
        slug: `gpt-new-${index}`,
        supported_in_api: true,
        visibility: "list",
      })),
    ];
    scenario.remoteSources.set(
      scenario.codexRawUrl("rust-v0.147.0", scenario.contract.codex.modelCatalogPath),
      json({ models }),
    );
    scenario.remoteSources.set(
      scenario.contract.claude.usageTypesSourceUrl,
      new Error("simulated late timeout"),
    );

    const report = await scenario.audit();
    const markdown = renderProviderAuditMarkdown(report);

    expect(report.status).toBe("unavailable");
    expect(report.findings.length).toBeLessThanOrEqual(50);
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "source-unavailable",
        provider: "claude",
        status: "unavailable",
      }),
    );
    expect(report.findings).toContainEqual(
      expect.objectContaining({
        code: "findings-truncated",
        provider: "codex",
      }),
    );
    expect(markdown.length).toBeLessThan(50_000);
    expect(markdown).not.toContain("gpt-new-998");
  });
});
