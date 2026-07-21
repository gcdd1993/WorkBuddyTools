import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.tsx", import.meta.url), "utf8");

function declarationsFor(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`));
  assert.ok(match, `Missing CSS rule for ${selector}`);
  return match[1];
}

function assertDeclaration(selector, declaration) {
  const escapedDeclaration = declaration
    .replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    .replace(/\s+/g, "\\s*");

  assert.match(
    declarationsFor(selector),
    new RegExp(escapedDeclaration),
    `${selector} should include ${declaration}`,
  );
}

test("prevents the document itself from becoming the scroll container", () => {
  assertDeclaration("html, body, #root", "height: 100%");
  assertDeclaration("html, body, #root", "overflow: hidden");
});

test("keeps the app shell pinned to the viewport and delegates overflow inward", () => {
  assertDeclaration(".app-shell", "height: 100dvh");
  assertDeclaration(".app-shell", "display: flex");
  assertDeclaration(".app-shell", "flex-direction: column");
  assertDeclaration(".app-shell", "overflow: hidden");
  assertDeclaration(".app-shell > .panel", "min-height: 0");
  assertDeclaration(".table-wrap", "flex: 1");
  assertDeclaration(".table-wrap", "overflow: auto");
});

test("keeps provider list and dialogs in dedicated interaction regions", () => {
  assertDeclaration(".provider-list", "flex: 1");
  assertDeclaration(".provider-list", "overflow: auto");
  assertDeclaration(".provider-list", "align-content: start");
  assertDeclaration(".provider-list", "grid-auto-rows: 84px");
  assertDeclaration(".provider-row", "height: 84px");
  assertDeclaration(".provider-row", "min-height: 84px");
  assertDeclaration(".modal-backdrop", "position: fixed");
  assertDeclaration(".modal-backdrop", "inset: 0");
  assertDeclaration(".provider-dialog", "max-width: 520px");
});

test("renders app prompts as a top-right toast layer outside the document flow", () => {
  assert.match(main, /className="toast-region"/);
  assert.doesNotMatch(main, /className="notice/);
  assertDeclaration(".toast-region", "position: fixed");
  assertDeclaration(".toast-region", "right: 24px");
  assertDeclaration(".toast-region", "z-index: 60");
  assertDeclaration(".toast", "box-shadow: var(--shadow-panel)");
});

test("keeps provider model rows aligned while allowing badges to wrap visibly", () => {
  assertDeclaration(".panel-header .primary-button", "white-space: nowrap");
  assertDeclaration(".model-choice", "align-items: flex-start");
  assertDeclaration(".model-choice", "overflow: visible");
  assertDeclaration(".model-choice-title-row", "display: flex");
  assertDeclaration(".model-choice-meta", "display: flex");
  assertDeclaration(".model-choice-meta", "flex-wrap: wrap");
  assertDeclaration(".model-choice-meta", "overflow: visible");
});
