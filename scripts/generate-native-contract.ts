type JsonSchema = {
  $defs?: Record<string, JsonSchema>;
  $ref?: string;
  const?: unknown;
  enum?: unknown[];
  format?: string;
  items?: JsonSchema;
  maxItems?: number;
  maximum?: number;
  minItems?: number;
  minimum?: number;
  maxLength?: number;
  minLength?: number;
  anyOf?: JsonSchema[];
  oneOf?: JsonSchema[];
  pattern?: string;
  properties?: Record<string, JsonSchema>;
  required?: string[];
  title?: string;
  type?: string | string[];
};

type NativeContractExport = {
  bootstrapContractVersion: number;
  bootstrapStateSchema: JsonSchema;
  contractVersion: number;
  doomerboardContractVersion: number;
  doomerboardViewSchema: JsonSchema;
  panelAddTokenmaxxerEvent: string;
  refreshReceiptSchema: JsonSchema;
  revisionNoticeEvent: string;
  revisionNoticeSchema: JsonSchema;
  settingsContractVersion: number;
  settingsNavigationEvent: string;
  settingsNavigationSchema: JsonSchema;
  settingsRecoveryClearEvent: string;
  settingsStateSchema: JsonSchema;
  stateSchema: JsonSchema;
  updateContractVersion: number;
  updateStateChangedEvent: string;
  updateStateSchema: JsonSchema;
};

const workspaceRoot = new URL("..", import.meta.url).pathname;
const process = Bun.spawn(
  [
    "cargo",
    "run",
    "--quiet",
    "--manifest-path",
    "apps/desktop/src-tauri/Cargo.toml",
    "--bin",
    "export_native_contract",
  ],
  { cwd: workspaceRoot, stderr: "inherit", stdout: "pipe" },
);
const contract = (await new Response(
  process.stdout,
).json()) as NativeContractExport;
if ((await process.exited) !== 0)
  throw new Error("native contract export failed");

const schema = contract.stateSchema;
const bootstrapStateSchema = contract.bootstrapStateSchema;
const doomerboardViewSchema = contract.doomerboardViewSchema;
const refreshReceiptSchema = contract.refreshReceiptSchema;
const revisionNoticeSchema = contract.revisionNoticeSchema;
const settingsNavigationSchema = contract.settingsNavigationSchema;
const settingsStateSchema = contract.settingsStateSchema;
const updateStateSchema = contract.updateStateSchema;
const definitions = {
  ...(bootstrapStateSchema.$defs ?? {}),
  ...(doomerboardViewSchema.$defs ?? {}),
  ...(refreshReceiptSchema.$defs ?? {}),
  ...(schema.$defs ?? {}),
  ...(revisionNoticeSchema.$defs ?? {}),
  ...(settingsNavigationSchema.$defs ?? {}),
  ...(settingsStateSchema.$defs ?? {}),
  ...(updateStateSchema.$defs ?? {}),
};
const schemaName = (name: string) =>
  `${name[0]!.toLowerCase()}${name.slice(1)}Schema`;
const refName = (ref: string) => schemaName(ref.split("/").at(-1)!);
const quoteKey = (key: string) =>
  /^[A-Za-z_$][\w$]*$/.test(key) ? key : JSON.stringify(key);

function pinContractVersion(node: JsonSchema, version: number): JsonSchema {
  return {
    ...node,
    ...(node.properties
      ? {
          properties: {
            ...Object.fromEntries(
              Object.entries(node.properties).map(([key, value]) => [
                key,
                pinContractVersion(value, version),
              ]),
            ),
            ...(node.properties.contractVersion
              ? { contractVersion: { const: version } }
              : {}),
          },
        }
      : {}),
    ...(node.anyOf
      ? {
          anyOf: node.anyOf.map((variant) =>
            pinContractVersion(variant, version),
          ),
        }
      : {}),
    ...(node.oneOf
      ? {
          oneOf: node.oneOf.map((variant) =>
            pinContractVersion(variant, version),
          ),
        }
      : {}),
  };
}

function render(node: JsonSchema, fieldName = ""): string {
  if (node.$ref) return refName(node.$ref);
  if ("const" in node) return `z.literal(${JSON.stringify(node.const)})`;
  if (node.enum) return `z.enum(${JSON.stringify(node.enum)})`;
  if (node.anyOf) {
    const nonNull = node.anyOf.filter((variant) => variant.type !== "null");
    if (nonNull.length === 1 && nonNull.length !== node.anyOf.length)
      return `${render(nonNull[0]!, fieldName)}.nullable()`;
    return `z.union([${node.anyOf.map((variant) => render(variant)).join(", ")}])`;
  }
  if (node.oneOf) {
    const nonNull = node.oneOf.filter((variant) => variant.type !== "null");
    if (nonNull.length === 1 && nonNull.length !== node.oneOf.length)
      return `${render(nonNull[0]!, fieldName)}.nullable()`;
    const discriminator = Object.keys(node.oneOf[0]?.properties ?? {}).find(
      (candidate) => {
        const values = node.oneOf!.map(
          (variant) => variant.properties?.[candidate]?.const,
        );
        return (
          values.every((value) => value !== undefined) &&
          new Set(values).size === values.length
        );
      },
    );
    const variants = node.oneOf.map((variant) => render(variant)).join(", ");
    return discriminator
      ? `z.discriminatedUnion(${JSON.stringify(discriminator)}, [${variants}])`
      : `z.union([${variants}])`;
  }
  if (Array.isArray(node.type)) {
    const withoutNull = node.type.filter((type) => type !== "null");
    return `${render({ ...node, type: withoutNull[0] }, fieldName)}.nullable()`;
  }
  if (node.type === "object") {
    const required = new Set(node.required ?? []);
    const fields = Object.entries(node.properties ?? {}).map(([key, value]) => {
      const expression = render(value, key);
      return `${quoteKey(key)}: ${required.has(key) ? expression : `${expression}.optional()`}`;
    });
    return `z.object({ ${fields.join(", ")} }).strict()`;
  }
  if (node.type === "array") {
    const item = render(node.items ?? {}, fieldName);
    if (node.maxItems === 0) return "z.tuple([])";
    if (node.minItems === node.maxItems && node.minItems !== undefined) {
      return `z.tuple([${Array.from({ length: node.minItems }, () => item).join(", ")}])`;
    }
    let expression = `z.array(${item})`;
    if (node.minItems !== undefined) expression += `.min(${node.minItems})`;
    if (node.maxItems !== undefined) expression += `.max(${node.maxItems})`;
    return expression;
  }
  if (node.type === "integer" || node.type === "number") {
    let expression = "z.number()";
    if (node.type === "integer") expression += ".int()";
    if (
      (node.minimum ?? Number.NEGATIVE_INFINITY) >= 0 ||
      fieldName.includes("Cost")
    ) {
      expression += ".nonnegative()";
    }
    if (node.minimum !== undefined && node.minimum > 0)
      expression += `.min(${node.minimum})`;
    if (node.maximum !== undefined) expression += `.max(${node.maximum})`;
    return expression;
  }
  if (node.type === "boolean") return "z.boolean()";
  if (node.type === "string") {
    if (fieldName === "revision") return "z.string().regex(/^[1-9]\\d*$/)";
    if (fieldName.endsWith("At")) return "z.string().datetime()";
    let expression = "z.string()";
    if (node.minLength !== undefined) expression += `.min(${node.minLength})`;
    if (node.maxLength !== undefined) expression += `.max(${node.maxLength})`;
    if (node.pattern !== undefined)
      expression += `.regex(new RegExp(${JSON.stringify(node.pattern)}))`;
    return expression;
  }
  return "z.unknown()";
}

function renderDefinition(name: string, node: JsonSchema): string {
  const expression = render(node);
  if (name !== "UsageTotal") return expression;
  return `${expression}.superRefine((value, context) => {
  if (value.availability === "unavailable") return;
  const cost = value.apiEquivalentCostUsd;
  const basis = value.apiEquivalentCostBasis;
  const quality = value.apiEquivalentCostQuality;
  const coverage = value.apiEquivalentCostCoveragePercent;
  const noCost = cost == null && basis == null && quality == null && coverage == null;
  const modeled = cost != null && basis != null && quality === "modeled" && coverage != null;
  const fixedQuality = cost != null && basis != null && (quality === "reconciled" || quality === "local-only") && coverage == null;
  if (!noCost && !modeled && !fixedQuality) {
    context.addIssue({ code: "custom", message: "invalid API-equivalent cost state" });
  }
})`;
}

function dependencies(node: JsonSchema): Set<string> {
  const refs = new Set<string>();
  const visit = (value: unknown) => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== "object") return;
    for (const [key, child] of Object.entries(value)) {
      if (key === "$ref" && typeof child === "string")
        refs.add(child.split("/").at(-1)!);
      else visit(child);
    }
  };
  visit(node);
  return refs;
}

const ordered: string[] = [];
const visiting = new Set<string>();
const visited = new Set<string>();
const visitDefinition = (name: string) => {
  if (visited.has(name)) return;
  if (visiting.has(name))
    throw new Error(`recursive native contract definition: ${name}`);
  visiting.add(name);
  for (const dependency of dependencies(definitions[name]!))
    visitDefinition(dependency);
  visiting.delete(name);
  visited.add(name);
  ordered.push(name);
};
for (const name of Object.keys(definitions).sort()) visitDefinition(name);

const generated = `// Generated by scripts/generate-native-contract.ts from the canonical Rust DTOs. Do not edit.
import * as z from "zod";

export const BOOTSTRAP_CONTRACT_VERSION = ${JSON.stringify(contract.bootstrapContractVersion)} as const;
export const CONTRACT_VERSION = ${JSON.stringify(contract.contractVersion)} as const;
export const DOOMERBOARD_CONTRACT_VERSION = ${JSON.stringify(contract.doomerboardContractVersion)} as const;
export const PANEL_ADD_TOKENMAXXER_EVENT = ${JSON.stringify(contract.panelAddTokenmaxxerEvent)} as const;
export const REVISION_NOTICE_EVENT = ${JSON.stringify(contract.revisionNoticeEvent)} as const;
export const SETTINGS_CONTRACT_VERSION = ${JSON.stringify(contract.settingsContractVersion)} as const;
export const SETTINGS_NAVIGATION_EVENT = ${JSON.stringify(contract.settingsNavigationEvent)} as const;
export const SETTINGS_RECOVERY_CLEAR_EVENT = ${JSON.stringify(contract.settingsRecoveryClearEvent)} as const;
export const UPDATE_CONTRACT_VERSION = ${JSON.stringify(contract.updateContractVersion)} as const;
export const UPDATE_STATE_CHANGED_EVENT = ${JSON.stringify(contract.updateStateChangedEvent)} as const;

${ordered.map((name) => `export const ${schemaName(name)} = ${renderDefinition(name, definitions[name]!)};`).join("\n")}
export const bootstrapStateSchema = ${render({ ...bootstrapStateSchema, properties: { ...bootstrapStateSchema.properties, contractVersion: { const: contract.bootstrapContractVersion } } })};
export const doomerboardViewSchema = ${render(pinContractVersion(doomerboardViewSchema, contract.doomerboardContractVersion))};
export const sanitizedDesktopStateSchema = ${render({ ...schema, properties: { ...schema.properties, contractVersion: { const: contract.contractVersion } } })};
export const refreshReceiptSchema = ${render(refreshReceiptSchema)};
export const revisionNoticeSchema = ${render(revisionNoticeSchema)};
export const settingsNavigationRequestSchema = ${render(settingsNavigationSchema)};
export const settingsStateSchema = ${render({ ...settingsStateSchema, properties: { ...settingsStateSchema.properties, contractVersion: { const: contract.settingsContractVersion } } })};
export const updateStateSchema = ${render({ ...updateStateSchema, properties: { ...updateStateSchema.properties, contractVersion: { const: contract.updateContractVersion } } })};

${ordered.map((name) => `export type ${name} = z.infer<typeof ${schemaName(name)}>;`).join("\n")}
export type BootstrapState = z.infer<typeof bootstrapStateSchema>;
export type DoomerboardView = z.infer<typeof doomerboardViewSchema>;
export type SanitizedDesktopState = z.infer<typeof sanitizedDesktopStateSchema>;
export type RefreshReceipt = z.infer<typeof refreshReceiptSchema>;
export type RevisionNotice = z.infer<typeof revisionNoticeSchema>;
export type SettingsNavigationRequest = z.infer<typeof settingsNavigationRequestSchema>;
export type SettingsState = z.infer<typeof settingsStateSchema>;
export type UpdateState = z.infer<typeof updateStateSchema>;
`;

const outputPath = `${workspaceRoot}packages/contracts/src/native.generated.ts`;
const output = generated.replaceAll(";export", ";\nexport");
if (Bun.argv.includes("--check")) {
  const current = await Bun.file(outputPath).text();
  if (current !== output)
    throw new Error("generated native contract is out of date");
} else {
  await Bun.write(outputPath, output);
}
