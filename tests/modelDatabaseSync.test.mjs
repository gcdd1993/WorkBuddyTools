import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  canonicalModelId,
  convertCherryModel,
  mergeCherryCatalog,
} from "../scripts/sync-cherry-model-database.mjs";

test("bundles the expanded Cherry model catalog", async () => {
  const database = JSON.parse(
    await readFile(new URL("../src-tauri/resources/modelDatabase.json", import.meta.url), "utf8"),
  );
  const ids = Object.keys(database).filter((id) => id !== "_meta");

  assert.ok(ids.length >= 850, `expected at least 850 models, received ${ids.length}`);
  assert.ok(database._meta.cherryStudio.modelCount >= 850);
  for (const id of ["claude-opus-4.6", "gemini-3-pro-preview", "gpt-5.4", "grok-4.5", "qwen-3-32b"]) {
    assert.ok(database[id], `missing representative model ${id}`);
  }
});

test("normalizes Cherry numeric hyphens to WorkBuddy dot variants", () => {
  assert.equal(canonicalModelId("x-ai/grok-4-5"), "x-ai/grok-4.5");
});

test("maps Cherry capabilities and supported reasoning efforts", () => {
  const converted = convertCherryModel({
    id: "example-2",
    name: "Example 2",
    capabilities: ["function-call", "reasoning", "image-recognition", "web-search"],
    contextWindow: 200000,
    maxOutputTokens: 64000,
    inputModalities: ["text", "image"],
    outputModalities: ["text"],
    reasoning: {
      controls: [{ kind: "effort", values: ["none", "low", "high"] }, { kind: "toggle" }],
      supportedEfforts: ["none", "auto", "low", "high"],
    },
  });

  assert.equal(converted.contextWindow, 200000);
  assert.equal(converted.maxOutput, 64000);
  assert.deepEqual(converted.reasoningEfforts, ["low", "high"]);
  assert.equal(converted.canDisableThinking, true);
  assert.equal(converted.onlyReasoning, false);
  assert.equal(converted.capabilities.vision, true);
  assert.equal(converted.capabilities.functionCalling, true);
  assert.equal(converted.capabilities.webSearch, true);
});

test("keeps official Grok 4.5 overrides while merging its Cherry alias", () => {
  const current = {
    _meta: { version: "old" },
    "grok-4.5": {
      contextWindow: 500000,
      maxOutput: 500000,
      capabilities: { reasoning: true },
      reasoningEfforts: ["low", "medium", "high", "xhigh"],
      defaultReasoningEffort: "high",
      canDisableThinking: false,
      onlyReasoning: true,
    },
  };
  const catalog = {
    version: "test",
    models: [{
      id: "grok-4-5",
      name: "Grok 4.5",
      capabilities: ["reasoning"],
      inputModalities: ["text", "image"],
      outputModalities: ["text"],
      reasoning: { controls: [{ kind: "effort", values: ["low", "medium", "high"] }] },
    }],
  };

  const { database } = mergeCherryCatalog(current, catalog);
  assert.equal(database["grok-4-5"], undefined);
  assert.deepEqual(database["grok-4.5"].reasoningEfforts, ["low", "medium", "high", "xhigh"]);
  assert.equal(database["grok-4.5"].canDisableThinking, false);
  assert.equal(database["grok-4.5"].onlyReasoning, true);
});
