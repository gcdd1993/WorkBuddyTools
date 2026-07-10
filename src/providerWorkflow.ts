export type ProviderWorkflowTab = "models" | "providers";

export type ProviderFormValue = {
  id?: string;
  name: string;
  baseUrl: string;
  apiKey: string;
};

export type ProviderFormSource = ProviderFormValue;

export function buildProviderForm(provider?: ProviderFormSource): ProviderFormValue {
  if (!provider) {
    return {
      name: "",
      baseUrl: "",
      apiKey: "",
    };
  }

  return {
    id: provider.id,
    name: provider.name,
    baseUrl: provider.baseUrl,
    apiKey: provider.apiKey,
  };
}

export function shouldAutoFetchProviderModels({
  activeTab,
  selectedProviderId,
  lastFetchedProviderId,
}: {
  activeTab: ProviderWorkflowTab;
  selectedProviderId: string;
  lastFetchedProviderId: string;
}) {
  return (
    activeTab === "providers" &&
    selectedProviderId.length > 0 &&
    selectedProviderId !== lastFetchedProviderId
  );
}
