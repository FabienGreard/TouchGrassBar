import type { BackendBinding } from "./evidence";

export const DEPLOYMENT_BINDING_PATH = "packages/backend/convex/internal/readinessDeployment.ts";

export function renderDeploymentBinding(binding: BackendBinding, productionDeployment: string) {
  return `import type { DeployedBackendBinding } from "./readinessDeploymentContract";

export const deployedBackendBinding: DeployedBackendBinding = {
  boardKeyVersion: ${JSON.stringify(binding.boardKeyVersion)},
  commit: ${JSON.stringify(binding.commit)},
  lockHash: ${JSON.stringify(binding.lockHash)},
  policyVersion: ${JSON.stringify(binding.policyVersion)},
  productionDeployment: ${JSON.stringify(productionDeployment)},
  schemaHash: ${JSON.stringify(binding.schemaHash)},
};
`;
}

export function deploymentBindingMatches(
  fileContents: string,
  binding: BackendBinding,
  productionDeployment: string,
) {
  return fileContents === renderDeploymentBinding(binding, productionDeployment);
}
