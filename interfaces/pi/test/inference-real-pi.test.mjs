import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("Pinned Pi native model selection updates footer state and routes each model through MagicMtn", { skip: !process.env.MAGICMTN_TEST_PI, timeout: 45000 }, async (t) => {
  const directory = mkdtempSync(join(tmpdir(), "magicmtn-real-pi-inference-"));
  const models = [
    { id: "5269bd6d-e31d-42e8-812f-989be2bb3e57", display_name: "Kimi K3", provider: "fireworks" },
    { id: "711189e2-b9a8-4888-b4f8-dcac5961bf69", display_name: "GLM Flash", provider: "baseten" },
  ];
  const sent = [];
  const server = createServer(async (request, response) => {
    let text = "";
    for await (const chunk of request) text += chunk;
    const body = JSON.parse(text);
    sent.push({ model: body.model, authorization: request.headers.authorization });
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    const chunk = { id: "mock", object: "chat.completion.chunk", created: 0, model: body.model };
    response.write(`data: ${JSON.stringify({ ...chunk, choices: [{ index: 0, delta: { role: "assistant", content: "Mock MagicMtn response" }, finish_reason: null }] })}\n\n`);
    response.write(`data: ${JSON.stringify({ ...chunk, choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } })}\n\n`);
    response.end("data: [DONE]\n\n");
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  const port = server.address().port;
  const log = join(directory, "calls.jsonl");
  writeFileSync(join(directory, "magic"), `#!${process.execPath}
import fs from 'node:fs';
const args=process.argv.slice(2);fs.appendFileSync(${JSON.stringify(log)},JSON.stringify(args)+'\\n');
const models=${JSON.stringify(models)};
let value;
if(args[1]==='account')value={signed_in:true};
else if(args[1]==='catalog')value={models,defaults:{model_id:models[0].id,inference_source:'magicmtn'}};
else if(args[1]==='inference'){
 const id=args[args.indexOf('--model')+1];const model=models.find(m=>m.id===id);
 if(!model)process.exit(2);
 value={baseUrl:'http://127.0.0.1:${port}/v1/harness/agent',api:'openai-completions',apiKey:'scoped-'+id,expiresIn:86400,
 models:[{id,name:model.display_name,reasoning:false,input:['text'],cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:32768,maxTokens:4096,compat:{supportsStore:false,supportsDeveloperRole:false,supportsReasoningEffort:false,maxTokensField:'max_tokens'}}]};
}else process.exit(3);
console.log(JSON.stringify(value));`, { mode: 0o755 });
  // Exercise the installed extension layout: no local node_modules; Pi must
  // provide its public SDK imports just as it does after magic pi install.
  const extensionDirectory = join(directory, "extension");
  mkdirSync(extensionDirectory);
  for (const filename of ["index.js", "inference.js", "picker.js", "workspace.js", "local.js"]) {
    copyFileSync(fileURLToPath(new URL(`../${filename}`, import.meta.url)), join(extensionDirectory, filename));
  }
  writeFileSync(join(extensionDirectory, "package.json"), '{"type":"module"}');
  const extension = join(extensionDirectory, "index.js");
  writeFileSync(extension, readFileSync(extension, "utf8").replace("const magicExecutable = null;", `const magicExecutable = ${JSON.stringify(join(directory, "magic"))};`));
  const child = spawn(process.env.MAGICMTN_TEST_PI, ["--mode", "rpc", "--no-session", "--offline", "--no-context-files", "--no-extensions", "-e", extension],
    { cwd: directory, env: { ...process.env, HOME: directory, PI_CODING_AGENT_DIR: join(directory, "agent"), PATH: `${directory}:${process.env.PATH}` }, stdio: ["pipe", "pipe", "pipe"] });
  t.after(() => { child.kill(); server.closeAllConnections(); server.close(); rmSync(directory, { recursive: true, force: true }); });
  let stderr = "";
  let buffer = "";
  let seq = 0;
  const pending = new Map();
  const events = new Map();
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.stdout.on("data", (chunk) => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, end); buffer = buffer.slice(end + 1);
      let value;
      try { value = JSON.parse(line); } catch { continue; }
      if (value.type === "response" && pending.has(value.id)) { pending.get(value.id)(value); pending.delete(value.id); }
      if (events.has(value.type)) { events.get(value.type)(value); events.delete(value.type); }
    }
  });
  const deadline = (promise) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`Pi RPC timeout: ${stderr}`)), 15000);
    promise.then((value) => { clearTimeout(timer); resolve(value); }, (error) => { clearTimeout(timer); reject(error); });
  });
  const ask = async (type, args = {}) => {
    const id = String(++seq);
    const response = deadline(new Promise((done) => pending.set(id, done)));
    child.stdin.write(`${JSON.stringify({ id, type, ...args })}\n`);
    const value = await response;
    assert.equal(value.success, true, JSON.stringify(value));
    return value.data;
  };
  const available = (await ask("get_available_models")).models.filter((model) => model.provider === "magicmtn");
  assert.equal(available.length, 2);
  assert.deepEqual(readFileSync(log, "utf8").trim().split("\n").map(JSON.parse), [["pi", "account"], ["pi", "catalog"]]);
  for (const [index, model] of available.entries()) {
    assert.match(model.id, /^(fireworks|baseten)\//);
    assert.notEqual(model.id, models[index].id, "the footer must show a readable model ID");
    await ask("set_model", { provider: "magicmtn", modelId: model.id });
    const state = await ask("get_state");
    assert.equal(state.model.provider, "magicmtn");
    assert.equal(state.model.id, model.id);
    const settled = deadline(new Promise((done) => events.set("agent_settled", done)));
    await ask("prompt", { message: "Use the local mock response." });
    await settled;
    assert.deepEqual(sent.at(-1), { model: models[index].id, authorization: `Bearer scoped-${models[index].id}` });
  }
  assert.equal(sent.length, 2);
  const messages = (await ask("get_messages")).messages;
  assert.ok(messages.some((message) => message.role === "assistant" && message.content?.some((part) => part.text === "Mock MagicMtn response")));
  const calls = readFileSync(log, "utf8").trim().split("\n").map(JSON.parse);
  assert.equal(calls.filter((args) => args[1] === "inference").length, 2);
  assert.ok(!calls.some((args) => ["create", "attach", "connect"].includes(args[1])));
});
