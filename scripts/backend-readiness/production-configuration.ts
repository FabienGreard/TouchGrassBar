type ProductionEnvironment = Record<string, string | undefined>;

export type ProductionConfiguration = {
  adminKey: string;
  deployment: {
    kind: "production";
    name: string;
    siteUrl: string;
    url: string;
  };
};

function required(environment: ProductionEnvironment, name: string) {
  const value = environment[name]?.trim();
  if (!value) throw new Error("Backend readiness requires the exact production deployment");
  return value;
}

function deploymentUrl(value: string, deploymentName: string, suffix: string) {
  const url = new URL(value);
  const hostnameMatches =
    url.hostname === `${deploymentName}.${suffix}` ||
    (url.hostname.startsWith(`${deploymentName}.`) && url.hostname.endsWith(`.${suffix}`));
  if (
    url.protocol !== "https:" ||
    !hostnameMatches ||
    url.username !== "" ||
    url.password !== "" ||
    url.port !== "" ||
    url.pathname !== "/" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    throw new Error("Backend readiness requires the exact production deployment");
  }
  return url.origin;
}

export function productionConfiguration(
  environment: ProductionEnvironment,
): ProductionConfiguration {
  const name = required(environment, "TOUCHGRASS_PRODUCTION_DEPLOYMENT");
  const adminKey = required(environment, "CONVEX_DEPLOY_KEY");
  const match = /^prod:([a-z0-9-]+)\|.+$/u.exec(adminKey);
  if (match?.[1] !== name) {
    throw new Error("Backend readiness requires the exact production deployment");
  }
  return {
    adminKey,
    deployment: {
      kind: "production",
      name,
      siteUrl: deploymentUrl(
        required(environment, "TOUCHGRASS_PRODUCTION_SITE_URL"),
        name,
        "convex.site",
      ),
      url: deploymentUrl(
        required(environment, "TOUCHGRASS_PRODUCTION_CONVEX_URL"),
        name,
        "convex.cloud",
      ),
    },
  };
}
