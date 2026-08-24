import { ConvexHttpClient } from "convex/browser";
import type { FunctionArgs, FunctionReference, FunctionReturnType } from "convex/server";

type AdminConvexHttpClient = {
  action<Action extends FunctionReference<"action", "internal">>(
    action: Action,
    args: FunctionArgs<Action>,
  ): Promise<FunctionReturnType<Action>>;
  mutation<Mutation extends FunctionReference<"mutation", "internal">>(
    mutation: Mutation,
    args: FunctionArgs<Mutation>,
  ): Promise<FunctionReturnType<Mutation>>;
  setAdminAuth(token: string): void;
};

export function adminClient(url: string, adminKey: string) {
  const client = new ConvexHttpClient(url) as unknown as AdminConvexHttpClient;
  client.setAdminAuth(adminKey);
  return client;
}
