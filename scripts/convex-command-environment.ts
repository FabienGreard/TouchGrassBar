const convexSelectionCommands = new Set([
  "deploy",
  "deployment",
  "login",
  "logout",
]);

function convexCommandEnvironment(
  argumentsList: string[],
  inheritedEnvironment: Record<string, string | undefined>,
  localEnvironment: Record<string, string | undefined>,
) {
  const environment = { ...inheritedEnvironment };
  delete environment.CONVEX_AGENT_MODE;
  const deployment =
    environment.CONVEX_DEPLOYMENT?.trim() ||
    localEnvironment.CONVEX_DEPLOYMENT?.trim();
  const command = argumentsList[0];
  if (
    deployment?.startsWith("anonymous:") &&
    command !== undefined &&
    !convexSelectionCommands.has(command)
  ) {
    environment.CONVEX_AGENT_MODE = "anonymous";
  }
  return environment;
}

export { convexCommandEnvironment };
