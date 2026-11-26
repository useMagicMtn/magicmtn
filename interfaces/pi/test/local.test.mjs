import assert from "node:assert/strict";
import test from "node:test";
import { localMenu } from "../local.js";

const model = { id: "model-id", display_name: "Chosen model", provider: "modal" };
const catalog = { models: [model], defaults: { model_id: model.id } };

test("new asks only for backend and attaches managed Pi using the prepared default image", async () => {
  const calls = [], terminals = [], menus = [];
  const request = async (args) => {
    calls.push(args);
    if (args[2] === "catalog") return catalog;
    if (args[1] === "images") return { images: [
      { name: "canonical", is_default: true, preparations: [{ provider: "modal", ready: true }, { provider: "apple-container", ready: true }] },
      { name: "custom", preparations: [{ provider: "custom-backend", ready: true }] },
    ] };
    return { id: "session-id" };
  };
  const ctx = { ui: { select: async (title, items) => { menus.push({title,items}); return "apple-container"; } } };
  await localMenu("new", ctx, request, async (_ctx,args) => terminals.push(args));
  assert.deepEqual(menus, [{title:"Sandbox backend",items:["modal","apple-container"]}]);
  assert.deepEqual(calls.at(-1), ["--local","pi","local-create","--provider","apple-container","--model","model-id"]);
  assert.deepEqual(terminals, [["--local","pi","attach","session-id"]]);
});

test("default selects and provisions the user's model without selecting one automatically", async () => {
  const calls = [];
  const request = async (args) => { calls.push(args); return args[2] === "catalog" ? catalog : {saved:true}; };
  const notices = [];
  await localMenu("default", {ui:{select:async (_title,items) => items[0],notify:(message) => notices.push(message)}}, request);
  assert.deepEqual(calls.at(-1),["--local","pi","defaults","--model","model-id"]);
  assert.match(notices.at(-1),/Default model: Chosen model/);
  calls.length=0;
  await localMenu("default", {ui:{select:async () => undefined}}, request);
  assert.equal(calls.length,1);
});

test("new requires preparation of the default and ignores prepared custom images", async () => {
  const calls = [], notices = [];
  const request = async (args) => {
    calls.push(args);
    if (args[2]==="catalog") return catalog;
    return {images:[{name:"canonical",is_default:true,preparations:[]},{name:"custom",preparations:[{provider:"modal",ready:true}]}]};
  };
  await localMenu("new", {ui:{notify:(message) => notices.push(message)}},request,() => assert.fail("no attach"));
  assert.equal(calls.length,2);
  assert.match(notices[0],/images prepare canonical --provider/);
});

test("view resumes a stopped session with its saved conversation", async () => {
  const calls=[],terminals=[];
  const session={id:"session-id",status:"stopped",provider:"modal",desired:"stopped"};
  const request=async (args) => {calls.push(args);return args[2]==="list" ? {workspaces:[session]} : session;};
  const choices=["session-id · modal · stopped","Resume"];
  await localMenu("view",{ui:{select:async () => choices.shift()}},request,async (_ctx,args) => terminals.push(args));
  assert.deepEqual(calls.slice(1),[["--local","pi","status","session-id"],["--local","pi","restart","session-id"]]);
  assert.deepEqual(terminals,[["--local","pi","attach","session-id"]]);
});

test("cancelling backend selection creates nothing", async () => {
  const calls=[];
  const request=async (args) => {calls.push(args);return args[2]==="catalog" ? catalog : {images:[{is_default:true,preparations:[{provider:"modal",ready:true},{provider:"apple-container",ready:true}]}]};};
  await localMenu("new",{ui:{select:async () => undefined}},request);
  assert.equal(calls.length,2);
});
