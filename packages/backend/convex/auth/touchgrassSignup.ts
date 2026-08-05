import type { BetterAuthPlugin } from "better-auth";
import {
  APIError,
  createAuthEndpoint,
  createAuthMiddleware,
} from "better-auth/api";

const PREPARATION_LIFETIME_MS = 120_000;
const PUBLIC_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PUBLIC_ID_PATTERN = /^TG-[A-HJ-NP-Z2-9]{6}$/;
const PROOF_HEADER = "x-touchgrass-signup-proof";

type PreparationPayload = {
  expiresAt: number;
  nonce: string;
  touchGrassId: string;
  version: 1;
};

function bytesToBase64Url(bytes: Uint8Array) {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/=+$/, "");
}

function base64UrlToBytes(value: string) {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
  const normalized = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  try {
    return Uint8Array.from(atob(padded), (character) =>
      character.charCodeAt(0),
    );
  } catch {
    return null;
  }
}

function randomBytes(length: number) {
  return crypto.getRandomValues(new Uint8Array(length));
}

function createTouchGrassId() {
  const suffix = [...randomBytes(6)]
    .map((byte) => PUBLIC_ID_ALPHABET[byte % PUBLIC_ID_ALPHABET.length])
    .join("");
  return `TG-${suffix}`;
}

function syntheticEmail(touchGrassId: string) {
  return `${touchGrassId.toLowerCase()}@identity.touchgrass.invalid`;
}

async function hmacKey(secret: string) {
  return crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["sign", "verify"],
  );
}

async function signPreparation(secret: string, payload: PreparationPayload) {
  const encodedPayload = bytesToBase64Url(
    new TextEncoder().encode(JSON.stringify(payload)),
  );
  const signature = await crypto.subtle.sign(
    "HMAC",
    await hmacKey(secret),
    new TextEncoder().encode(encodedPayload),
  );
  return `${encodedPayload}.${bytesToBase64Url(new Uint8Array(signature))}`;
}

async function verifyPreparation(
  secret: string,
  proof: string,
  now: number,
) {
  if (proof.length > 1_024) return null;
  const [encodedPayload, encodedSignature, extra] = proof.split(".");
  if (!encodedPayload || !encodedSignature || extra !== undefined) return null;
  const payloadBytes = base64UrlToBytes(encodedPayload);
  const signature = base64UrlToBytes(encodedSignature);
  if (!payloadBytes || !signature) return null;
  const validSignature = await crypto.subtle.verify(
    "HMAC",
    await hmacKey(secret),
    signature,
    new TextEncoder().encode(encodedPayload),
  );
  if (!validSignature) return null;

  try {
    const value = JSON.parse(new TextDecoder().decode(payloadBytes)) as unknown;
    if (!value || typeof value !== "object") return null;
    const payload = value as Partial<PreparationPayload>;
    if (
      payload.version !== 1 ||
      typeof payload.expiresAt !== "number" ||
      !Number.isSafeInteger(payload.expiresAt) ||
      payload.expiresAt <= now ||
      typeof payload.nonce !== "string" ||
      payload.nonce.length < 20 ||
      typeof payload.touchGrassId !== "string" ||
      !PUBLIC_ID_PATTERN.test(payload.touchGrassId)
    ) {
      return null;
    }
    return payload as PreparationPayload;
  } catch {
    return null;
  }
}

function signupProofFromHeaders(context: {
  headers?: Headers | undefined;
  request?: { headers: Headers } | undefined;
}) {
  return (
    context.request?.headers.get(PROOF_HEADER) ??
    context.headers?.get(PROOF_HEADER) ??
    null
  );
}

export function touchGrassSignup(): BetterAuthPlugin {
  return {
    id: "touchgrass-signup",
    endpoints: {
      prepareTouchGrassSignup: createAuthEndpoint(
        "/touchgrass/prepare",
        { method: "POST" },
        async (ctx) => {
          const touchGrassId = createTouchGrassId();
          const payload: PreparationPayload = {
            expiresAt: Date.now() + PREPARATION_LIFETIME_MS,
            nonce: bytesToBase64Url(randomBytes(16)),
            touchGrassId,
            version: 1,
          };
          return ctx.json({
            expiresAt: payload.expiresAt,
            signupProof: await signPreparation(ctx.context.secret, payload),
            touchGrassId,
          });
        },
      ),
    },
    hooks: {
      before: [
        {
          matcher: (context) => context.path === "/sign-up/email",
          handler: createAuthMiddleware(async (ctx) => {
            const proof = signupProofFromHeaders(ctx);
            const preparation = proof
              ? await verifyPreparation(ctx.context.secret, proof, Date.now())
              : null;
            if (
              !preparation ||
              ctx.body.username !== preparation.touchGrassId ||
              ctx.body.email !== syntheticEmail(preparation.touchGrassId)
            ) {
              throw new APIError("FORBIDDEN", {
                message: "Signup preparation is invalid or expired",
              });
            }
          }),
        },
      ],
    },
    rateLimit: [
      {
        max: 5,
        pathMatcher: (path) => path === "/touchgrass/prepare",
        window: 60,
      },
    ],
  };
}
