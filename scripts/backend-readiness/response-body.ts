export async function readBoundedResponseText(
  response: Response,
  maximumBytes: number,
  errorMessage: string,
) {
  const declaredLength = response.headers.get("content-length");
  if (
    declaredLength !== null &&
    (!/^\d+$/u.test(declaredLength) || Number(declaredLength) > maximumBytes)
  ) {
    await response.body?.cancel();
    throw new Error(errorMessage);
  }
  if (response.body === null) return { byteLength: 0, text: "" };

  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let byteLength = 0;
  let text = "";
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      byteLength += chunk.value.byteLength;
      if (byteLength > maximumBytes) {
        await reader.cancel();
        throw new Error(errorMessage);
      }
      text += decoder.decode(chunk.value, { stream: true });
    }
    text += decoder.decode();
  } catch (error) {
    await reader.cancel().catch(() => undefined);
    if (error instanceof Error && error.message === errorMessage) throw error;
    // oxlint-disable-next-line preserve-caught-error -- The remote body error can contain private response data.
    throw new Error(errorMessage);
  }
  return { byteLength, text };
}
