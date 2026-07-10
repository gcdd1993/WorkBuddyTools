import test from "node:test";
import assert from "node:assert/strict";

import {
  buildProviderForm,
  shouldAutoFetchProviderModels,
} from "../.tmp-tests/providerWorkflow.js";

test("auto-fetches models only when the providers tab has a newly selected provider", () => {
  assert.equal(
    shouldAutoFetchProviderModels({
      activeTab: "providers",
      selectedProviderId: "provider-a",
      lastFetchedProviderId: "",
    }),
    true,
  );

  assert.equal(
    shouldAutoFetchProviderModels({
      activeTab: "models",
      selectedProviderId: "provider-a",
      lastFetchedProviderId: "",
    }),
    false,
  );

  assert.equal(
    shouldAutoFetchProviderModels({
      activeTab: "providers",
      selectedProviderId: "",
      lastFetchedProviderId: "",
    }),
    false,
  );

  assert.equal(
    shouldAutoFetchProviderModels({
      activeTab: "providers",
      selectedProviderId: "provider-a",
      lastFetchedProviderId: "provider-a",
    }),
    false,
  );
});

test("builds a provider form for add or edit dialogs", () => {
  assert.deepEqual(buildProviderForm(), {
    name: "",
    baseUrl: "",
    apiKey: "",
  });

  assert.deepEqual(
    buildProviderForm({
      id: "p1",
      name: "随时跑路",
      baseUrl: "https://runanytime.hx.im/v1",
      apiKey: "sk-test",
    }),
    {
      id: "p1",
      name: "随时跑路",
      baseUrl: "https://runanytime.hx.im/v1",
      apiKey: "sk-test",
    },
  );
});
