import assert from "node:assert/strict";
import test from "node:test";
import { localMenu } from "../local.js";

test("new selects prepared custom images and passes image and provider to creation", async () => {
  const calls = [];
  const request = async (args, _signal, body) => {
    calls.push({ args, body });
    if (args[1] === "images") return { images: [
      { name: "canonical", is_default: true, preparations: [{ provider: "modal", ready: false }] },
      { name: "my-dev", preparations: [{ provider: "modal", ready: true }] },
    ] };
    return { content: [{ text: JSON.stringify({ id: "sandbox-id" }) }] };
  };
  const notices = [];
  const ctx = { ui: { notify: (message) => notices.push(message), select: () => assert.fail("one ready selection needs no picker") } };
  await localMenu("new", ctx, request, () => assert.fail("no terminal takeover"));
  assert.deepEqual(calls[0].args, ["--local", "images", "list", "--format", "json"]);
  assert.deepEqual(calls[1].body, { image: "my-dev", provider: "modal" });
  assert.match(notices[0], /Created sandbox sandbox-id/);
});

test("new explains how to prepare an image without creating a sandbox", async () => {
  const calls = [];
  const notices = [];
  const request = async (args) => { calls.push(args); return { images: [{ name: "canonical", preparations: [] }] }; };
  await localMenu("new", { ui: { notify: (message) => notices.push(message) } }, request);
  assert.equal(calls.length, 1);
  assert.match(notices[0], /images prepare canonical --provider/);
});

test("cancelling image selection performs no sandbox creation", async () => {
  const calls = [];
  const request = async (args) => {
    calls.push(args);
    return { images: [{ name: "canonical", preparations: [{ provider: "modal", ready: true }, { provider: "apple-container", ready: true }] }] };
  };
  await localMenu("new", { ui: { select: async () => undefined } }, request);
  assert.equal(calls.length, 1);
});
