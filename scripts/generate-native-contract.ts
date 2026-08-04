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
  minLength?: number;
  oneOf?: JsonSchema[];
  properties?: Record<string, JsonSchema>;
  required?: string[];
  title?: string;
  type?: string | string[];
};

type NativeContractExport = {
  contractVersion: number;
  refreshReceiptSchema: JsonSchema;
  revisionNoticeEvent: string;
  revisionNoticeSchema: JsonSchema;
  stateSchema: JsonSchema;
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
const contract = (await new Response(process.stdout).json()) as NativeContractExport;
if ((await process.exited) !== 0)
  throw new Error("native contract export failed");

const schema = contract.stateSchema;
const refreshReceiptSchema = contract.refreshReceiptSchema;
const revisionNoticeSchema = contract.revisionNoticeSchema;
const definitions = {
  ...(refreshReceiptSchema.$defs ?? {}),
  ...(schema.$defs ?? {}),
  ...(revisionNoticeSchema.$defs ?? {}),
};
const schemaName = (name: string) =>
  `${name[0]!.toLowerCase()}${name.slice(1)}Schema`;
const refName = (ref: string) => schemaName(ref.split("/").at(-1)!);
const quoteKey = (key: string) =>
  /^[A-Za-z_$][\w$]*$/.test(key) ? key : JSON.stringify(key);

function render(node: JsonSchema, fieldName = ""): string {
  if (node.$ref) return refName(node.$ref);
  if ("const" in node) return `z.literal(${JSON.stringify(node.const)})`;
  if (node.enum) return `z.enum(${JSON.stringify(node.enum)})`;
  if (node.oneOf) {
    const discriminator = node.oneOf
      .map(
        (variant) =>
          Object.entries(variant.properties ?? {}).find(
            ([, value]) => "const" in value,
          )?.[0],
      )
      .find(Boolean);
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
    return expression;
  }
  if (node.type === "boolean") return "z.boolean()";
  if (node.type === "string") {
    if (fieldName === "revision") return "z.string().regex(/^[1-9]\\d*$/)";
    if (fieldName.endsWith("At")) return "z.string().datetime()";
    let expression = "z.string()";
    if (node.minLength !== undefined) expression += `.min(${node.minLength})`;
    return expression;
  }
  return "z.unknown()";
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

export const CONTRACT_VERSION = ${JSON.stringify(contract.contractVersion)} as const;
export const REVISION_NOTICE_EVENT = ${JSON.stringify(contract.revisionNoticeEvent)} as const;

${ordered.map((name) => `export const ${schemaName(name)} = ${render(definitions[name]!)};`).join("\n")}
export const sanitizedDesktopStateSchema = ${render({ ...schema, properties: { ...schema.properties, contractVersion: { const: contract.contractVersion } } })};
export const refreshReceiptSchema = ${render(refreshReceiptSchema)};
export const revisionNoticeSchema = ${render(revisionNoticeSchema)};

${ordered.map((name) => `export type ${name} = z.infer<typeof ${schemaName(name)}>;`).join("\n")}
export type SanitizedDesktopState = z.infer<typeof sanitizedDesktopStateSchema>;
export type RefreshReceipt = z.infer<typeof refreshReceiptSchema>;
export type RevisionNotice = z.infer<typeof revisionNoticeSchema>;
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
