type WebMcpTextContent = {
  text: string;
  type: "text";
};

type WebMcpTool = {
  description: string;
  execute: () => { content: WebMcpTextContent[] };
  inputSchema: {
    additionalProperties: false;
    properties: Record<string, never>;
    type: "object";
  };
  name: string;
};

type ModelContext = {
  registerTool(tool: WebMcpTool): Promise<void> | void;
};

type WebMcpDocument = Document & {
  modelContext?: ModelContext;
};

const downloadTool: Omit<WebMcpTool, "execute"> = {
  description: "Start the current approved TouchGrassBar download for macOS.",
  inputSchema: {
    additionalProperties: false,
    properties: {},
    type: "object",
  },
  name: "download-touchgrassbar-for-macos",
};

export async function installWebMcp(documentObject: Document) {
  const modelContext = (documentObject as WebMcpDocument).modelContext;
  if (!modelContext?.registerTool) return false;

  try {
    await modelContext.registerTool({
      ...downloadTool,
      execute: () => {
        const downloadLink = documentObject.querySelector<HTMLAnchorElement>(
          "[data-download-link]",
        );

        if (!downloadLink) {
          return {
            content: [{
              text: "The TouchGrassBar download link is not available on this page.",
              type: "text",
            }],
          };
        }

        downloadLink.click();
        return {
          content: [{
            text: "The approved TouchGrassBar download for macOS has started.",
            type: "text",
          }],
        };
      },
    });
    return true;
  } catch {
    return false;
  }
}
