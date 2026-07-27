import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const CHERRY_CATALOG_URL =
  "https://raw.githubusercontent.com/CherryHQ/cherry-studio/main/packages/provider-registry/data/models.json";
const CHERRY_CONTENTS_API_URL =
  "https://api.github.com/repos/CherryHQ/cherry-studio/contents/packages/provider-registry/data/models.json?ref=main";
const CHERRY_SOURCE = "cherry-studio";
const VALID_REASONING_EFFORTS = new Set([
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
]);

// These entries have been checked against vendor documentation and must take
// precedence when Cherry's generated catalog temporarily lags behind.
const OFFICIAL_OVERRIDES = new Set(["grok-4.5"]);

export function canonicalModelId(modelId) {
  const characters = modelId.trim().toLowerCase().split("");
  return characters
    .map((character, index) => {
      const previous = characters[index - 1];
      const next = characters[index + 1];
      return (character === "-" || character === ".") && /\d/.test(previous ?? "") && /\d/.test(next ?? "")
        ? "."
        : character;
    })
    .join("");
}

function numericPrice(pricing, field) {
  const value = pricing?.[field]?.perMillionTokens;
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function reasoningMetadata(model) {
  if (!model.reasoning || typeof model.reasoning !== "object") return {};

  const controls = Array.isArray(model.reasoning.controls) ? model.reasoning.controls : [];
  const declaredEfforts = Array.isArray(model.reasoning.supportedEfforts)
    ? model.reasoning.supportedEfforts
    : [];
  const controlEfforts = controls
    .filter((control) => control?.kind === "effort" && Array.isArray(control.values))
    .flatMap((control) => control.values);
  const supportedEfforts = [...new Set([...declaredEfforts, ...controlEfforts])].filter((effort) =>
    VALID_REASONING_EFFORTS.has(effort),
  );
  const effortControl = controls.find((control) => control?.kind === "effort");
  const defaultEffort = model.reasoning.defaultEffort ?? effortControl?.default;
  const hasToggle = controls.some((control) => control?.kind === "toggle");
  const result = {};

  if (supportedEfforts.length > 0) result.reasoningEfforts = supportedEfforts;
  if (VALID_REASONING_EFFORTS.has(defaultEffort)) result.defaultReasoningEffort = defaultEffort;
  if (hasToggle) {
    result.canDisableThinking = true;
    result.onlyReasoning = false;
  }
  return result;
}

export function convertCherryModel(model) {
  const capabilities = new Set(Array.isArray(model.capabilities) ? model.capabilities : []);
  const inputModalities = new Set(Array.isArray(model.inputModalities) ? model.inputModalities : []);
  const outputModalities = new Set(Array.isArray(model.outputModalities) ? model.outputModalities : []);
  const entry = {
    displayName: model.name || model.id,
  };

  if (Number.isFinite(model.contextWindow)) entry.contextWindow = model.contextWindow;
  if (Number.isFinite(model.maxOutputTokens)) entry.maxOutput = model.maxOutputTokens;

  entry.capabilities = {
    vision: capabilities.has("image-recognition") || inputModalities.has("image"),
    functionCalling: capabilities.has("function-call"),
    reasoning: capabilities.has("reasoning"),
    streaming: outputModalities.has("text"),
    webSearch: capabilities.has("web-search"),
  };
  if (capabilities.has("image-generation") || outputModalities.has("image")) {
    entry.capabilities.imageGeneration = true;
  }

  Object.assign(entry, reasoningMetadata(model));

  const input = numericPrice(model.pricing, "input");
  const output = numericPrice(model.pricing, "output");
  if (input !== undefined || output !== undefined) {
    entry.pricing = {};
    if (input !== undefined) entry.pricing.input = input;
    if (output !== undefined) entry.pricing.output = output;
  }
  entry.catalogSource = CHERRY_SOURCE;
  return entry;
}

function mergeCapabilities(base, incoming, incomingWins) {
  return incomingWins ? { ...base, ...incoming } : { ...incoming, ...base };
}

function mergeEntry(existing, incoming, officialOverride) {
  const incomingWins = !officialOverride;
  const merged = incomingWins ? { ...existing, ...incoming } : { ...incoming, ...existing };
  merged.capabilities = mergeCapabilities(
    existing.capabilities ?? {},
    incoming.capabilities ?? {},
    incomingWins,
  );
  // An overlapping pre-existing row remains locally curated. Only rows created
  // wholly from Cherry are marked as generated and replaceable on future syncs.
  delete merged.catalogSource;
  return merged;
}

export function mergeCherryCatalog(database, catalog) {
  const result = {};
  for (const [id, entry] of Object.entries(database)) {
    if (id !== "_meta" && entry?.catalogSource !== CHERRY_SOURCE) result[id] = entry;
  }

  const idsByCanonical = new Map(
    Object.keys(result).map((id) => [canonicalModelId(id), id]),
  );
  let added = 0;
  let updated = 0;

  for (const model of catalog.models ?? []) {
    if (!model?.id) continue;
    const canonical = canonicalModelId(model.id);
    const existingId = idsByCanonical.get(canonical);
    const converted = convertCherryModel(model);
    if (existingId) {
      result[existingId] = mergeEntry(
        result[existingId],
        converted,
        OFFICIAL_OVERRIDES.has(existingId),
      );
      updated += 1;
    } else {
      result[model.id] = converted;
      idsByCanonical.set(canonical, model.id);
      added += 1;
    }
  }

  const meta = {
    ...(database._meta ?? {}),
    version: new Date().toISOString().slice(0, 10),
    cherryStudio: {
      catalogVersion: catalog.version ?? "unknown",
      modelCount: Array.isArray(catalog.models) ? catalog.models.length : 0,
      repository: "https://github.com/CherryHQ/cherry-studio/tree/main/packages/provider-registry",
    },
  };
  const sorted = Object.fromEntries(
    Object.entries(result).sort(([left], [right]) => left.localeCompare(right)),
  );
  return { database: { _meta: meta, ...sorted }, added, updated };
}

async function fetchCherryCatalog() {
  const headers = { Accept: "application/json", "User-Agent": "WorkBuddyTools" };
  try {
    const response = await fetch(CHERRY_CATALOG_URL, {
      headers,
      signal: AbortSignal.timeout(30_000),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  } catch (rawError) {
    const response = await fetch(CHERRY_CONTENTS_API_URL, {
      headers,
      signal: AbortSignal.timeout(30_000),
    });
    if (!response.ok) {
      throw new Error(`Cherry catalog download failed: ${rawError}; GitHub API HTTP ${response.status}`);
    }
    const payload = await response.json();
    return JSON.parse(Buffer.from(payload.content.replace(/\n/g, ""), "base64").toString("utf8"));
  }
}

async function main() {
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const databasePath = path.join(repositoryRoot, "src-tauri", "resources", "modelDatabase.json");
  const database = JSON.parse(await readFile(databasePath, "utf8"));
  const catalog = await fetchCherryCatalog();
  const merged = mergeCherryCatalog(database, catalog);
  await writeFile(databasePath, `${JSON.stringify(merged.database, null, 2)}\n`, "utf8");
  process.stdout.write(
    `Cherry catalog ${catalog.version}: ${catalog.models.length} models, ${merged.added} added, ${merged.updated} updated\n`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  await main();
}
