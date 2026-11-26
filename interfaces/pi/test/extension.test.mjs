import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, readFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import workspaceStatus from "../workspace.js";
import { DefaultPicker } from "../picker.js";
import { getKeybindings } from "@earendil-works/pi-tui";

const connection = { id: "7f0b4271-bf76-4de0-8e76-ddd1b0e5ee13", display_name: "Remote", provider: "exe", capabilities: { sandbox_operations: ["create", "exec_bounded", "terminate"] } };
const model = { id: "5269bd6d-e31d-42e8-812f-989be2bb3e57", display_name: "Model", provider: "fireworks" };
const workspace = { id: "d215125b-4896-4ca4-a36f-aefdeea095fb", provider: "exe", status: "running", created_at: "2026-10-14" };

async function fixture(t, { image = true, workspaces = [], failCreate = false, failAttach = false, interruptAttach = false, removeCliBeforeAttach = false, signedIn = true, local = false, localImages = [{name:"canonical",is_default:true,preparations:[{provider:"modal",ready:true}]}], models = [model], connections = [connection], defaults = {} } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "magicmtn-extension-"));
  const log = join(directory, "calls.jsonl");
  writeFileSync(log, "");
  const accountPath = join(directory, "account.json");
  const catalogPath = join(directory, "catalog.json");
  const workspacesPath = join(directory, "workspaces.json");
  writeFileSync(workspacesPath, JSON.stringify(workspaces));
  writeFileSync(catalogPath, JSON.stringify({ models, defaults: { model_id: models[0]?.id, ...defaults }, default_image: image ? { display_name: "MagicMtn Canonical" } : null }));
  const account = { signed_in: true, user: { display_name: "Rodrigo", email: "rodrigo@example.test" } };
  writeFileSync(accountPath, JSON.stringify(local ? {mode:"local",signed_in:false} : signedIn ? account : { signed_in: false }));
  const originalPath = process.env.PATH;
  const impostor = join(directory, "impostor");
  // Every test, including terminal attachment, must bypass a hostile PATH.
  mkdirSync(impostor);
  writeFileSync(join(impostor, "magic"), "#!/bin/sh\nexit 99\n", { mode: 0o755 });
  process.env.PATH = `${impostor}:${originalPath}`;
  const cli = `#!${process.execPath}
import fs from 'node:fs';
let args=process.argv.slice(2);fs.appendFileSync(${JSON.stringify(log)},JSON.stringify(args)+'\\n');
if(args[0]==='--local')args=args.slice(1);
let result;
if(args[0]==='mcp' && args[1]==='tools')result=[{name:'sandbox_list',description:'List local sandboxes',inputSchema:{type:'object',properties:{}}},{name:'sandbox_create',description:'Create local sandbox',inputSchema:{type:'object',properties:{image:{type:'string'}}}}];
if(args[0]==='mcp' && args[1]==='call'){const input=JSON.parse(fs.readFileSync(0,'utf8'));const value=args[2]==='sandbox_list'?JSON.parse(fs.readFileSync(${JSON.stringify(workspacesPath)},'utf8')):${JSON.stringify(workspace)};result={content:[{type:'text',text:JSON.stringify(value)}],details:{}};}
if(args[1]==='attach'){
  if(${interruptAttach})process.kill(process.pid,'SIGTERM');
  if(${failAttach}){
    console.error('Connecting workspace…');
    console.error('Error: server returned 400 Bad Request: sandbox is not available');
    process.exit(1);
  }
  process.exit(0);
}
if(args[1]==='connect')result={id:'pi-connection'};
if(['status','stop','restart'].includes(args[1]))result={state:'running',generation:2};
if(args[1]==='account')result=JSON.parse(fs.readFileSync(${JSON.stringify(accountPath)},'utf8'));
if(args[1]==='catalog')result=JSON.parse(fs.readFileSync(${JSON.stringify(catalogPath)},'utf8'));
if(args[1]==='defaults'){const c=JSON.parse(fs.readFileSync(${JSON.stringify(catalogPath)},'utf8'));c.defaults={model_id:args[args.indexOf('--model')+1]};fs.writeFileSync(${JSON.stringify(catalogPath)},JSON.stringify(c));result={saved:true}};
if(args[1]==='terminate'){const items=JSON.parse(fs.readFileSync(${JSON.stringify(workspacesPath)},'utf8'));fs.writeFileSync(${JSON.stringify(workspacesPath)},JSON.stringify(items.filter(w=>w.id!==args[2])));result={status:'ended'}};
if(args[1]==='list')result={workspaces:JSON.parse(fs.readFileSync(${JSON.stringify(workspacesPath)},'utf8'))};
if(args[0]==='images' && args[1]==='list')result={images:${JSON.stringify(localImages)}};
if(['create','local-create'].includes(args[1])){if(${failCreate}){console.error('Creation uncertain; inspect workspace');process.exit(1)}if(${removeCliBeforeAttach})fs.unlinkSync(process.argv[1]);result=${JSON.stringify(workspace)}}
if(args[1]==='inference')result={baseUrl:'https://magic.test/v1/pi/agent',apiKey:'scoped',api:'openai-completions',models:[{...JSON.parse(fs.readFileSync(${JSON.stringify(catalogPath)},'utf8')).models.find(m=>m.id===args[args.indexOf('--model')+1]),name:'Managed model',contextWindow:65536,maxTokens:8192}],expiresIn:86400};
console.log(JSON.stringify(result));`;
  writeFileSync(join(directory, "magic"), cli, { mode: 0o755 });
  writeFileSync(join(directory, "package.json"), '{"type":"module"}');
  t.after(() => { process.env.PATH = originalPath; rmSync(directory, { recursive: true, force: true }); });
  const calls = () => readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
  const notices = [];
  const choices = [];
  const menus = [];
  const terminalEvents = [];
  const statuses = new Map();
  let command;
  let provider;
  let available = false;
  const handlers = new Map();
  const tools = new Map();
  let activeTools = ["read", "bash"];
  const pi = {
    on(event, handler) { handlers.set(event, handler); },
    registerTool(tool) { tools.set(tool.name,tool); activeTools=[...new Set([...activeTools,tool.name])]; },
    getActiveTools() { return activeTools; },
    setActiveTools(names) { activeTools=names; },
    registerCommand(name, value) { assert.equal(name, "magic"); command = value; },
    registerProvider(value) { provider = value; available = false; },
    unregisterProvider(name) { assert.equal(name, "magicmtn"); provider = undefined; },
    async setModel(value) {
      assert.ok(available, "await registry refresh before selecting a model");
      ctx.model = value;
      handlers.get("model_select")?.({ model: value }, ctx);
      return true;
    },
  };
  const index = join(directory, "index.js");
  let source = readFileSync(new URL("../index.js", import.meta.url), "utf8")
    .replace("const magicExecutable = null;", `const magicExecutable = ${JSON.stringify(join(directory, "magic"))};`);
  for (const name of ["workspace.js", "picker.js", "inference.js", "local.js"]) {
    source = source.replace(`"./${name}"`, JSON.stringify(new URL(`../${name}`, import.meta.url).href));
  }
  writeFileSync(index, source);
  const { default: extension } = await import(pathToFileURL(index));
  extension(pi);
  const ctx = {
    mode: "tui", async waitForIdle() {},
    model: undefined,
    modelRegistry: {
      async refresh() { available = true; },
      find(name, id) { return available && provider?.id === name ? provider.getModels().find((entry) => entry.id === id) : undefined; },
    },
    ui: {
      async input() { return choices.shift(); },
      async select(title, items) { menus.push({ title, items }); const next = choices.shift(); return typeof next === "number" ? items[next] : next; },
      notify(message, type) { notices.push({ message, type }); },
      setStatus(key, value) { statuses.set(key, value); },
      async confirm() { return true; },
      async custom(factory) {
        let result;
        const component = factory({ stop() { terminalEvents.push("stop"); }, start() { terminalEvents.push("start"); }, requestRender() {} }, { fg(_color, text) { return text; } }, getKeybindings(), (value) => { result = value; });
        if (component instanceof DefaultPicker) {
          menus.push({ title: component.title, items: component.items.map((item) => item.label), index: component.index, rendered: component.render(100).join("\n") });
          const choice = choices.shift();
          if (choice === undefined) component.handleInput("\x1b");
          else if (choice === "<left>") component.handleInput("\x1b[D");
          else if (typeof choice === "function") choice(component);
          else { component.index = typeof choice === "number" ? choice : component.items.findIndex((item) => item.label === choice); component.handleInput("\r"); }
        }
        return result;
      },
    },
  };
  return { ctx, command, choices, menus, notices, terminalEvents, statuses, calls, handlers, tools, activeTools: () => activeTools, setAccount: (account) => writeFileSync(accountPath, JSON.stringify(account)), provider: () => provider };
}

test("signed-in menu shows the account name without authentication options", async (t) => {
  const f = await fixture(t);
  await f.command.handler("", f.ctx);
  assert.equal(f.menus[0].title, "MagicMtn · Rodrigo");
  assert.ok(!f.menus[0].items.includes("Sign out"));
  assert.ok(!f.menus[0].items.includes("Sign in"));
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
  assert.equal(f.menus[0].items[0], "Default (Model)");
  assert.deepEqual(f.menus[0].items, ["Default (Model)", "View computers", "New workspace", "Inference only"]);
  assert.ok(!f.menus[0].items.includes("Open workspace"));
  assert.ok(!f.menus[0].items.includes("Terminate workspace"));
});
test("Default uses account catalogs, chooses only a model, and saves without provisioning", async (t) => {
  const baseten = { ...model, id: "baseten-model", display_name: "GLM 5.3 Flash", provider: "baseten" };
  const apple = { ...connection, id: "apple", provider: "apple-container" };
  const modal = { ...connection, id: "modal", provider: "modal" };
  const f = await fixture(t, { models: [model, baseten], connections: [apple, connection, modal] });
  f.choices.push(1);
  await f.command.handler("default", f.ctx);
  assert.equal(f.menus[0].title, "Choose default model");
  assert.match(f.menus[0].rendered, /Model Name:/);
  assert.equal(f.menus.length, 1);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "defaults", "--model", baseten.id]]);
  assert.ok(f.notices.some(({message}) => message.includes("/magic new") && message.includes("/magic view")));
  await f.command.handler("", f.ctx);
  assert.equal(f.menus.at(-1).items[0], "Default (GLM 5.3 Flash)");
});
test("model pickers deduplicate backend display names and preserve the saved default", async (t) => {
  const duplicate = { ...model, id: "baseten-model", provider: "baseten" };
  const f = await fixture(t, { models: [model, duplicate], defaults: { model_id: duplicate.id } });
  await f.handlers.get("session_start")({}, f.ctx);
  assert.equal(f.provider().getModels().length, 1);
  assert.match(f.provider().getModels()[0].id, /^baseten\//);
  f.choices.push(0);
  await f.command.handler("default", f.ctx);
  assert.deepEqual(f.menus.at(-1).items, ["Model [baseten]"]);
  assert.match(f.menus.at(-1).rendered, /· default/);
  assert.deepEqual(f.calls().at(-1), ["pi", "defaults", "--model", duplicate.id]);
  f.choices.push(0);
  await f.command.handler("inference", f.ctx);
  assert.equal(f.menus.at(-1).items.length, 1);
  assert.deepEqual(f.calls().at(-1), ["pi", "inference", "--model", duplicate.id]);
});

test("cancelling the model picker preserves defaults", async (t) => {
  const f = await fixture(t);
  f.choices.push(undefined);
  await f.command.handler("default", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
});
test("Left from default model returns to the main menu", async (t) => {
  const f = await fixture(t);
  f.choices.push("<left>", undefined);
  await f.command.handler("default", f.ctx);
  assert.equal(f.menus[1].title, "MagicMtn · Rodrigo");
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "catalog"]]);
});
test("sandbox and inference lists can return to the main menu with Left", async (t) => {
  const f = await fixture(t, { workspaces: [workspace] });
  for (const action of ["view", "inference"]) {
    f.choices.push("<left>", undefined);
    await f.command.handler(action, f.ctx);
    assert.equal(f.menus.at(-1).title, "MagicMtn · Rodrigo");
  }
  assert.ok(!f.calls().some((args) => ["attach", "inference", "terminate"].includes(args[1])));
});
test("Left from termination confirmation returns to sandbox actions, then to the list", async (t) => {
  const f = await fixture(t, { workspaces: [workspace] });
  f.choices.push(0, 1, "<left>", "<left>", undefined);
  await f.command.handler("view", f.ctx);
  assert.deepEqual(f.menus[3].items, ["Resume", "Terminate", "Stop agent", "Restart agent"]);
  assert.equal(f.menus[4].title, "Sandboxes");
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"], ["pi", "list"]]);
});
test("signed-out menu directs users to the CLI without opening a picker", async (t) => {
  const f = await fixture(t, { signedIn: false });
  await f.command.handler("", f.ctx);
  assert.deepEqual(f.menus, []);
  assert.deepEqual(f.terminalEvents, []);
  assert.deepEqual(f.calls(), [["pi", "account"]]);
  assert.deepEqual(f.notices, [{ message: "Run magic login in your terminal, then use /magic.", type: "info" }]);
});
test("creates the default workspace without selection menus and hands the terminal to its real pi", async (t) => {
  const f = await fixture(t);
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "create", "--model", model.id], ["pi", "attach", workspace.id]]);
  assert.deepEqual(f.terminalEvents, ["stop", "start"]);
  assert.match(f.statuses.get("magicmtn-workspace"), /Local pi.*disconnected from MagicMtn computer d215125b/);
  assert.deepEqual(f.menus, []);
});
test("relay actions are rejected without provisioning or taking over the terminal", async (t) => {
  const f = await fixture(t);
  for (const action of ["relay", "Relay current directory"]) {
    await f.command.handler(action, f.ctx);
    assert.equal(f.notices.at(-1).message, "Usage: /magic [default|view|new|inference]");
  }
  assert.deepEqual(f.calls(), []);
  assert.deepEqual(f.terminalEvents, []);
  assert.deepEqual(f.menus, []);
});

test("new workspace cannot provision without a MagicMtn model", async (t) => {
  const f = await fixture(t, { models: [] });
  f.choices.push(0);
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
  assert.match(f.notices.at(-1).message, /No default inference model/);
});
test("new without saved defaults uses managed inference even with a current Pi model", async (t) => {
  const f = await fixture(t);
  f.ctx.model = { id: "local-kimi", provider: "moonshot", api: "openai-completions" };
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "create", "--model", model.id], ["pi", "attach", workspace.id]]);
});
test("no eligible managed default does not silently connect the current Pi model", async (t) => {
  const f = await fixture(t, { models: [] });
  f.ctx.model = { id: "local-kimi", provider: "moonshot", api: "openai-completions" };
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
  assert.match(f.notices.at(-1).message, /No default inference model/);
});
test("new uses saved defaults even when they are not first in the catalog", async (t) => {
  const secondModel = { ...model, id: "second-model" };
  const secondConnection = { ...connection, id: "second-connection" };
  const f = await fixture(t, { models: [model, secondModel], connections: [connection, secondConnection], defaults: { inference_source: "magicmtn", model_id: secondModel.id, connection_id: secondConnection.id } });
  f.ctx.model = { id: "local-model", provider: "local" };
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls()[2], ["pi", "create", "--model", secondModel.id]);
  assert.deepEqual(f.menus, []);
});
test("new connects the current Pi model when Pi is the default inference source", async (t) => {
  const f = await fixture(t, { defaults: { inference_source: "pi" } });
  f.ctx.model = { id: "local-model", provider: "local", api: "openai-completions" };
  f.ctx.modelRegistry.isUsingOAuth = () => false;
  f.ctx.modelRegistry.getApiKeyAndHeaders = async () => ({ ok: true, apiKey: "test-key" });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "connect"], ["pi", "create", "--pi-connection", "pi-connection"], ["pi", "attach", workspace.id]]);
  assert.deepEqual(f.menus, []);
});
test("unexpected local Pi extension errors report only the operation", async (t) => {
  const f = await fixture(t, { defaults: { inference_source: "pi" } });
  f.ctx.model = { id: "local-model", provider: "local", api: "openai-completions" };
  f.ctx.modelRegistry.isUsingOAuth = () => { throw new Error("private registry detail"); };
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [
    ["pi", "account"], ["pi", "catalog"],
    ["pi", "report-error", "--operation", "create"],
  ]);
  assert.match(f.notices.at(-1).message, /private registry detail/);
});
test("an interrupted attach reports the attach operation without terminal output", async (t) => {
  const f = await fixture(t, { interruptAttach: true });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls().at(-2), ["pi", "attach", workspace.id]);
  assert.deepEqual(f.calls().at(-1), ["pi", "report-error", "--operation", "attach"]);
  assert.match(f.notices.at(-1).message, /magic was interrupted/);
});
test("an attach rejected by the server shows the CLI error after restoring Pi", async (t) => {
  const f = await fixture(t, { workspaces: [workspace], failAttach: true });
  f.choices.push(0, 0);
  await f.command.handler("view", f.ctx);
  assert.deepEqual(f.terminalEvents, ["stop", "start"]);
  assert.deepEqual(f.calls().at(-1), ["pi", "attach", workspace.id]);
  assert.deepEqual(f.notices.at(-1), {
    message: `Could not connect to workspace ${workspace.id}. server returned 400 Bad Request: sandbox is not available`,
    type: "error",
  });
});
test("a missing magic executable shows the launch error", async (t) => {
  const f = await fixture(t, { removeCliBeforeAttach: true });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls().map((args) => args[1]), ["account", "catalog", "create"]);
  assert.match(f.notices.at(-1).message, /refresh its saved path/);
});
test("new workspace delegates placement without a backend catalog", async (t) => {
  const f = await fixture(t, { connections: [] });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls()[2], ["pi", "create", "--model", model.id]);
});
test("open lists every live instance across backends without grouping", async (t) => {
  const second = { ...workspace, id: "second-apple", provider: "apple-container" };
  const third = { ...second, id: "third-apple" };
  const ended = { ...workspace, id: "ended", status: "ended" };
  const f = await fixture(t, { workspaces: [workspace, second, third, ended] });
  f.choices.push(2, 0);
  await f.command.handler("open", f.ctx);
  assert.equal(f.menus[0].items.length, 3);
  assert.doesNotMatch(f.menus[0].items.join(" "), /exe.dev|Apple Container/);
  assert.match(f.menus[0].items[1], /MagicMtn Canonical/);
  assert.deepEqual(f.menus[1].items, ["Resume", "Terminate", "Stop agent", "Restart agent"]);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"], ["pi", "attach", third.id]]);
});
test("open with no live workspaces does not start provisioning", async (t) => {
  const f = await fixture(t);
  await f.command.handler("open", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"]]);
  assert.match(f.notices.at(-1).message, /No live workspaces/);
});
test("Resume is first and reconnects the selected workspace", async (t) => {
  const f = await fixture(t, { workspaces: [workspace] });
  f.choices.push("View computers", 0, (picker) => picker.handleInput("\r"));
  await f.command.handler("", f.ctx);
  assert.equal(f.menus[2].items[0], "Resume");
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "list"], ["pi", "attach", workspace.id]]);
});
test("missing default image and uncertain creation never fall through to attach", async (t) => {
  const f = await fixture(t, { image: false });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
  assert.match(f.notices[0].message, /published and enabled/);
});
test("an uncertain create is surfaced and is not automatically retried", async (t) => {
  const f = await fixture(t, { failCreate: true });
  f.choices.push(0, 0);
  await f.command.handler("new", f.ctx);
  assert.equal(f.calls().filter((call) => call[1] === "create").length, 1);
  assert.equal(f.calls().filter((call) => call[1] === "attach").length, 0);
  assert.match(f.notices.at(-1).message, /uncertain/);
});
test("inference-only registers a scoped provider without creating a VM", async (t) => {
  const f = await fixture(t);
  f.choices.push(0);
  await f.command.handler("inference", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "inference", "--model", model.id]]);
  assert.equal(f.provider().id, "magicmtn");
  assert.equal(f.ctx.model.provider, "magicmtn");
  assert.equal(f.ctx.model.id, "fireworks/model:5269bd6d");
  assert.equal(f.ctx.model.contextWindow, 65536);
  assert.match(f.statuses.get("magicmtn"), /local tools.*Model \(Fireworks\)/);
});

test("VM footer names its computer and follows inference model changes", (t) => {
  const previous = process.env.MAGICMTN_WORKSPACE_INFO;
  t.after(() => {
    if (previous === undefined) delete process.env.MAGICMTN_WORKSPACE_INFO;
    else process.env.MAGICMTN_WORKSPACE_INFO = previous;
  });
  process.env.MAGICMTN_WORKSPACE_INFO = JSON.stringify({ id: workspace.id, provider: "apple-container", inference_provider: "fireworks" });
  const handlers = new Map();
  workspaceStatus({ on(event, handler) { handlers.set(event, handler); } });
  let status;
  const ctx = { model: { id: model.id, name: "GLM 5.3 Flash", provider: "magicmtn" }, ui: { setStatus(_key, value) { status = value; } } };
  handlers.get("session_start")({}, ctx);
  assert.equal(status, "MagicMtn · computer d215125b · GLM 5.3 Flash (MagicMtn)");
  ctx.model = { id: "another", name: "Direct model", provider: "anthropic" };
  handlers.get("model_select")({}, ctx);
  assert.match(status, /Direct model \(anthropic\)/);
  assert.doesNotMatch(status, /Fireworks/);
});

test("termination uses the existing workspace lifecycle", async (t) => {
  const f = await fixture(t, { workspaces: [workspace] });
  f.choices.push(0, 1, 1);
  await f.command.handler("view", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"], ["pi", "terminate", workspace.id], ["pi", "list"]]);
  assert.ok(f.notices.some((notice) => /ended/.test(notice.message)));
});

test("terminating one sandbox refreshes the list and preserves other sandboxes", async (t) => {
  const other = { ...workspace, id: "711189e2-b9a8-4888-b4f8-dcac5961bf69", provider: "apple-container" };
  const f = await fixture(t, { workspaces: [workspace, other] });
  f.choices.push(0, 1, 1, undefined);
  await f.command.handler("view", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"], ["pi", "terminate", workspace.id], ["pi", "list"]]);
  assert.equal(f.menus.at(-1).title, "Sandboxes");
  assert.equal(f.menus.at(-1).items.length, 1);
  assert.match(f.menus.at(-1).rendered, /711189e2/);
});

test("cancelling termination leaves the sandbox available without entering it", async (t) => {
  const f = await fixture(t, { workspaces: [workspace] });
  f.choices.push(0, 1, 0, undefined);
  await f.command.handler("view", f.ctx);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "list"]]);
  assert.deepEqual(f.menus.at(-1).items, ["Resume", "Terminate", "Stop agent", "Restart agent"]);
});

test("authentication commands do not invoke the CLI or take over the terminal", async (t) => {
  const f = await fixture(t);
  for (const action of ["login", "logout", "Sign in", "Sign out"]) {
    await f.command.handler(action, f.ctx);
    assert.equal(f.notices.at(-1).message, "Usage: /magic [default|view|new|inference]");
  }
  assert.deepEqual(f.calls(), []);
  assert.deepEqual(f.terminalEvents, []);
  assert.deepEqual(f.menus, []);
});

for (const [label,command] of [["Stop agent","stop"],["Restart agent","restart"]]) {
 test(`workspace ${command} uses explicit session lifecycle command`, async (t) => {
  const f = await fixture(t, {workspaces:[workspace]});
  f.choices.push(0,label,undefined);
  await f.command.handler("view",f.ctx);
  assert.ok(f.calls().some(args => args[1] === command && args[2] === workspace.id));
  assert.ok(!f.calls().some(args => ["create","terminate","attach"].includes(args[1])));
 });
}

test("startup lists all signed-in MagicMtn models without changing the current model or granting inference", async (t) => {
  const second = { ...model, id: "second-model", display_name: "Second model" };
  const f = await fixture(t, { models: [model, second] });
  const original = { provider: "amazon-bedrock", id: "unavailable-default" };
  f.ctx.model = original;
  await f.handlers.get("session_start")({}, f.ctx);
  assert.deepEqual(f.provider().getModels().map((entry) => entry.id), ["fireworks/model:5269bd6d", "fireworks/second-model:second-m"]);
  assert.equal((await f.provider().auth.apiKey.check()).type, "api_key");
  assert.equal(f.ctx.model, original);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"]]);
});

test("inference selection updates Pi's actual current model and keeps the full catalog", async (t) => {
  const second = { ...model, id: "second-model", display_name: "Second model" };
  const f = await fixture(t, { models: [model, second] });
  f.ctx.model = { provider: "amazon-bedrock", id: "unavailable-default" };
  f.choices.push(1);
  await f.command.handler("inference", f.ctx);
  assert.equal(f.ctx.model.provider, "magicmtn");
  assert.equal(f.ctx.model.id, "fireworks/second-model:second-m");
  assert.deepEqual(f.provider().getModels().map((entry) => entry.id), ["fireworks/model:5269bd6d", "fireworks/second-model:second-m"]);
  assert.deepEqual(f.calls(), [["pi", "account"], ["pi", "catalog"], ["pi", "inference", "--model", second.id]]);
  assert.match(f.statuses.get("magicmtn"), /Second model/);
  await f.handlers.get("model_select")({ model: { provider: "anthropic", id: "direct" } }, f.ctx);
  assert.equal(f.statuses.get("magicmtn"), undefined);
});

test("signed-out startup does not advertise managed models or request their catalog", async (t) => {
  const f = await fixture(t, { signedIn: false });
  await f.handlers.get("session_start")({}, f.ctx);
  assert.equal(f.provider(), undefined);
  assert.deepEqual(f.calls(), [["pi", "account"]]);
});

test("local startup keeps Pi's own model and loads sandbox tools without Cloud catalogs", async (t) => {
  const f = await fixture(t, { local: true });
  const native = { id: "my-model", provider: "my-provider", api: "openai-responses" };
  f.ctx.model = native;
  await f.handlers.get("session_start")({}, f.ctx);
  assert.equal(f.ctx.model, native);
  assert.equal(f.provider(), undefined);
  assert.deepEqual(f.calls(), [["pi","account"],["--local","mcp","tools"]]);
  assert.match(f.statuses.get("magicmtn"), /Local/);
  assert.ok(f.tools.has("magic_sandbox_list"));
  assert.deepEqual(f.notices, []);
  await f.tools.get("magic_sandbox_list").execute("tool-id", {});
  assert.deepEqual(f.calls().at(-1), ["--local","mcp","call","sandbox_list"]);
});

test("local menu creates managed Pi and attaches its terminal", async (t) => {
  const f = await fixture(t, { local: true });
  await f.command.handler("new", f.ctx);
  assert.deepEqual(f.calls(), [["pi","account"],["--local","mcp","tools"],["--local","pi","catalog"],["--local","images","list","--format","json"],["--local","pi","local-create","--provider","modal","--model",model.id],["--local","pi","attach",workspace.id]]);
  assert.ok(f.terminalEvents.includes("stop"));
  assert.ok(f.terminalEvents.includes("start"));
});

test("View computers lists local managed sessions", async (t) => {
  const f = await fixture(t, { local: true, workspaces: [workspace] });
  f.choices.push(1);
  await f.command.handler("", f.ctx);
  assert.deepEqual(f.menus[0].items, ["New computer", "View computers", "Default model"]);
  assert.deepEqual(f.menus[1].items, [`${workspace.id} · exe · running`]);
  assert.deepEqual(f.calls().at(-1), ["--local", "pi", "list"]);
});

test("local default provisions the chosen provider model without a Cloud login", async (t) => {
  const f = await fixture(t, { local: true });
  f.choices.push(0);
  await f.command.handler("default", f.ctx);
  assert.deepEqual(f.calls().slice(-2), [["--local","pi","catalog"],["--local","pi","defaults","--model",model.id]]);
  assert.match(f.notices.at(-1).message,/Default model: Model/);
});

test("mode changes disable local tools only when the CLI selects Cloud again", async (t) => {
  const f = await fixture(t, { local: true });
  await f.handlers.get("session_start")({}, f.ctx);
  await f.handlers.get("before_agent_start")({}, f.ctx);
  assert.ok(f.activeTools().includes("magic_sandbox_list"));
  assert.ok(!f.calls().some((args) => args[1] === "catalog"));
  f.setAccount({signed_in:true, user:{display_name:"Person"}});
  await f.handlers.get("before_agent_start")({}, f.ctx);
  assert.ok(!f.activeTools().includes("magic_sandbox_list"));
  assert.ok(f.activeTools().includes("bash"));
  assert.ok(f.provider());
});
