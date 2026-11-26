import { spawn, spawnSync } from "node:child_process";
import { stripVTControlCharacters } from "node:util";
import { providerLabel } from "./workspace.js";
import { BACK, pickDefault } from "./picker.js";
import { createInferenceProvider, uniqueModels } from "./inference.js";
import { registerLocalTools, localMenu } from "./local.js";

// Replaced with the installing CLI's absolute path by magic pi install.
const magicExecutable = null;
function executable() {
  if (!magicExecutable) throw new Error("Run magic pi install to bind this extension to your CLI.");
  return magicExecutable;
}

// The CLI alone owns account credentials and provider setup. Pi sees only a
// short-lived inference grant when explicitly requested by the user.
function consumerEnvironment() {
  const env = { ...process.env };
  delete env.MAGICMTN_SERVER_URL;
  return env;
}

export function magic(args, signal, input) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable(), args, { stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"], signal, env: consumerEnvironment() });
    if (input !== undefined) {
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(input));
    }
    let output = "";
    let errors = "";
    child.stdout.setEncoding("utf8").on("data", (data) => {
      output += data;
      if (output.length > 4 * 1024 * 1024) child.kill();
    });
    child.stderr.setEncoding("utf8").on("data", (data) => {
      errors = (errors + data).slice(-16000);
    });
    child.on("error", (error) => reject(new Error(error.code === "ENOENT" ? "Reinstall the magic CLI and run magic pi install to refresh its saved path." : error.message)));
    child.on("close", (code) => {
      if (code !== 0) {
        const error = new Error(errors.trim() || "MagicMtn command failed");
        error.reported = true; // The CLI reports its own Pi command failures.
        return reject(error);
      }
      try { resolve(JSON.parse(output)); } catch { reject(new Error("Invalid response from magic CLI")); }
    });
  });
}

export function workspaceModels(ctx, catalog) {
  const models = uniqueModels(catalog.models, catalog.defaults?.model_id);
  if (ctx.model && ctx.model.provider !== "magicmtn") {
    models.unshift({ id: "__pi_current__", display_name: `Pi: ${ctx.model.name || ctx.model.id}`, provider: ctx.model.provider });
  }
  return models;
}

export async function connectPiModel(ctx, connect = (value) => magic(["pi", "connect"], undefined, value)) {
  const model = ctx.model;
  if (!model || !["openai-completions", "openai-responses", "anthropic-messages"].includes(model.api)) {
    throw new Error("This Pi model API is not supported by detached inference yet. Select a supported HTTP API or a MagicMtn model.");
  }
  if (ctx.modelRegistry.isUsingOAuth(model)) {
    throw new Error("Detached OAuth inference needs a gateway account connection. Use an API-key provider for now; your local OAuth credentials will not be copied.");
  }
  if (!(await ctx.ui.confirm("Connect Pi inference", `Allow MagicMtn's trusted gateway to store your ${model.provider} API key for remote sessions? Real keys stay outside the sandbox. This does not copy your other Pi credentials.`))) {
    throw new Error("Pi inference connection cancelled");
  }
  const auth = await ctx.modelRegistry.getApiKeyAndHeaders(model);
  if (!auth.ok || !auth.apiKey || (auth.headers && Object.keys(auth.headers).length) || (auth.env && Object.keys(auth.env).length)) {
    throw new Error("This Pi authentication configuration requires a dedicated gateway adapter. No credentials were uploaded.");
  }
  return connect({ model: { ...model, baseUrl: auth.baseUrl || model.baseUrl }, api_key: auth.apiKey });
}

function screenState(states, key) {
  if (!states.has(key)) states.set(key, {});
  return states.get(key);
}

async function select(ctx, title, items, label, includeId = true, states = new Map()) {
  if (!items.length) throw new Error(`No ${title.toLowerCase()} available. Configure a provider connection in MagicMtn.`);
  const chosen = await pickDefault(ctx, { title, state: screenState(states, title),
    items: items.map((item) => ({ ...item, label: includeId ? `${label(item)} [${item.id}]` : label(item), detail: label(item) })),
  });
  return chosen === BACK || chosen === undefined ? chosen : items.find((item) => item.id === chosen.id);
}

async function selectModel(ctx, catalog, states) {
  if (!catalog.models.length && catalog.unavailable_models?.length) {
    throw new Error(catalog.unavailable_models.map((m) => `${m.display_name}: ${m.reason}`).join("\n"));
  }
  return select(ctx, "Models", uniqueModels(catalog.models, catalog.defaults.model_id), (m) => `${m.display_name} (${m.provider})`, true, states);
}

async function terminal(ctx, args) {
  const attaching = args[0] === "pi" && args[1] === "attach";
  let result;
  const connected = await ctx.ui.custom((tui, _theme, _keys, done) => {
    tui.stop();
    try {
      // The child owns the real terminal. No RPC UI emulation or duplicate agent
      // loop; after disconnect, pi restores its original local interface.
      result = spawnSync(executable(), args, {
        stdio: attaching ? ["inherit", "inherit", "pipe"] : "inherit",
        encoding: "utf8",
        env: consumerEnvironment(),
      });
    } finally {
      tui.start();
      tui.requestRender(true);
    }
    done(result?.status === 0);
    return { render: () => [], invalidate() {} };
  });
  if (result?.error || result?.signal) {
    const detail = result.error?.code === "ENOENT" ? "Reinstall the magic CLI and run magic pi install to refresh its saved path."
      : result.error?.message || `magic was interrupted by ${result.signal}`;
    const error = new Error(detail);
    error.attachFailure = attaching;
    throw error;
  }
  if (attaching && !connected) {
    // Pi redraws over the child’s terminal after attach exits. Preserve the
    // CLI error in Pi while leaving the remote PTY on inherited stdout.
    const diagnostic = stripVTControlCharacters(result.stderr || "")
      .split(/\r?\n/).find((line) => line.startsWith("Error: "));
    const detail = diagnostic?.slice(7).replace(/[\x00-\x1f\x7f-\x9f]/g, "").trim().slice(0, 500)
      || `magic pi attach exited with status ${result.status}`;
    const error = new Error(`Could not connect to workspace ${args[2]}. ${detail}`);
    error.reported = true; // The CLI reports its own Pi command failures.
    throw error;
  }
  return connected;
}

export default function magicmtn(pi) {
  const inference = createInferenceProvider(magic);
  let mode;
  let localTools = [];
  const synchronizeMode = async (ctx, account) => {
    const next = account.mode === "local" ? "local" : "cloud";
    if (next === "local") {
      inference.clear();
      pi.unregisterProvider("magicmtn");
      if (mode !== "local") localTools = await registerLocalTools(pi, magic);
      ctx.ui.setStatus("magicmtn", "MagicMtn · Local");
    } else if (mode === "local") {
      pi.setActiveTools(pi.getActiveTools().filter((name) => !localTools.includes(name)));
      ctx.ui.setStatus("magicmtn", undefined);
    }
    mode = next;
    return next;
  };
  const refreshModels = async (ctx, catalog) => {
    inference.load(catalog.models, catalog.defaults.model_id);
    pi.registerProvider(inference.provider);
    // Pi's available-model/auth snapshots refresh asynchronously. Await them
    // before setModel(), and before offering these models in its native picker.
    await ctx.modelRegistry.refresh({ allowNetwork: false, providers: ["magicmtn"] });
  };
  const updateStatus = (model, ctx) => {
    if (mode === "local") {
      ctx.ui.setStatus("magicmtn", "MagicMtn · Local");
      return;
    }
    const managed = model?.provider === "magicmtn" && inference.model(model.id);
    ctx.ui.setStatus("magicmtn", managed
      ? `MagicMtn · local tools · ${managed.display_name} (${providerLabel(managed.provider)})`
      : undefined);
  };
  pi.on("session_start", async (_event, ctx) => {
    try {
      const account = await magic(["pi", "account"]);
      if (await synchronizeMode(ctx, account) === "local") {
        return;
      }
      if (account.signed_in) {
        await refreshModels(ctx, await magic(["pi", "catalog"]));
        updateStatus(ctx.model, ctx);
      } else {
        inference.clear();
        pi.unregisterProvider("magicmtn");
      }
    } catch {
      inference.clear();
      pi.unregisterProvider("magicmtn");
      if (ctx.model?.provider === "magicmtn") ctx.ui.notify("Could not load MagicMtn models. Check magic login, then use /magic inference.", "warning");
    }
  });
  pi.on("before_agent_start", async (_event, ctx) => {
    const account = await magic(["pi", "account"]);
    const previous = mode;
    await synchronizeMode(ctx, account);
    if (previous === "local" && mode === "cloud" && account.signed_in) {
      await refreshModels(ctx, await magic(["pi", "catalog"]));
      updateStatus(ctx.model, ctx);
    }
  });
  pi.on("model_select", (event, ctx) => updateStatus(event.model || ctx.model, ctx));
  pi.registerCommand("magic", {
    description: "Open a MagicMtn pi workspace, or configure MagicMtn inference",
    handler: async (args, ctx) => {
      if (ctx.mode !== "tui") throw new Error("/magic requires pi's interactive terminal mode");
      await ctx.waitForIdle();
      let account;
      const states = new Map();
      let action = args.trim();
      if (["View sandboxes", "View computers"].includes(action)) action = "view";
      if (!["", "default", "inference", "Inference only", "new", "New workspace", "New sandbox", "view", "open"].includes(action)) {
        ctx.ui.notify("Usage: /magic [default|view|new|inference]", "info");
        return;
      }
      navigation: while (true) {
        try {
          let catalog;
          account ??= await magic(["pi", "account"]);
          await synchronizeMode(ctx, account);
          if (mode === "local") {
            await localMenu(action, ctx, magic, terminal);
            return;
          }
          if (!account.signed_in) {
            ctx.ui.notify("Run magic login in your terminal, then use /magic.", "info");
            return;
          }
          if (!action) {
            catalog = await magic(["pi", "catalog"]);
            const defaultModel = catalog && (catalog.defaults.inference_source === "pi" && ctx.model
              ? { display_name: `Pi: ${ctx.model.name || ctx.model.id}` }
              : catalog.models.find((m) => m.id === catalog.defaults.model_id));
            const defaultLabel = defaultModel
              ? `Default (${defaultModel.display_name})`
              : "Default (configure model)";
            const actions = [
              { id: "default", label: defaultLabel },
              { id: "view", label: "View computers" },
              { id: "new", label: "New workspace" },
              { id: "inference", label: "Inference only" },
            ];
            const selected = await select(ctx, `MagicMtn · ${account.user.display_name}`, actions, (item) => item.label, false, states);
            if (selected === BACK || !selected) return;
            action = selected.id;
          }
          if (!action) return;
          if (action === "default") {
            catalog ??= await magic(["pi", "catalog"]);
            while (true) {
              const model = await pickDefault(ctx, {
                kind: "Model", defaultId: catalog.defaults.inference_source === "pi" && ctx.model ? "__pi_current__" : catalog.defaults.model_id, state: screenState(states, "default-model"),
                items: workspaceModels(ctx, catalog).map((m) => ({ ...m, label: `${m.display_name} [${m.provider}]`, detail: m.display_name })),
              });
              if (model === BACK) { action = ""; continue navigation; }
              if (!model) return;
              await magic(["pi", "defaults", ...(model.id === "__pi_current__" ? ["--use-pi"] : ["--model", model.id])]);
              ctx.ui.notify(`Workspace defaults saved: ${model.display_name}. Use /magic new to start a workspace, or /magic view to reconnect.`, "info");
              return;
            }
          }
          if (["inference", "Inference only"].includes(action)) {
            const catalog = await magic(["pi", "catalog"]);
            const model = await selectModel(ctx, catalog, states);
            if (model === BACK) { action = ""; continue navigation; }
            if (!model) return;
            await refreshModels(ctx, catalog);
            await inference.authorize(model.id);
            await ctx.modelRegistry.refresh({ allowNetwork: false, providers: ["magicmtn"] });
            const registered = ctx.modelRegistry.find("magicmtn", inference.idFor(model.id));
            if (!registered || !(await pi.setModel(registered))) throw new Error("Could not select the MagicMtn model");
            ctx.ui.setStatus("magicmtn", `MagicMtn · local tools · ${model.display_name} (${providerLabel(model.provider)})`);
            ctx.ui.notify("MagicMtn inference enabled with local tools. Model access renews automatically. Pricing is not yet reported in Pi.", "info");
            return;
          }
          let workspace;
          if (["open", "view"].includes(action)) {
            sandboxes: while (true) {
              const { workspaces } = await magic(["pi", "list"]);
              const running = workspaces.filter((w) => ["running", "starting"].includes(w.status));
              if (!running.length) {
                ctx.ui.notify("No live workspaces. Use /magic new to create one.", "info");
                return;
              }
              workspace = await select(ctx, "Sandboxes", running, (w) => `${w.image?.display_name || "MagicMtn Canonical"} · ${w.created_at}`, true, states);
              if (workspace === BACK) { action = ""; continue navigation; }
              if (!workspace) return;
              while (true) {
                const selected = await select(ctx, `MagicMtn sandbox ${workspace.id.slice(0, 8)}`,
                  [{ id: "resume", label: "Resume" }, { id: "terminate", label: "Terminate" }, { id: "stop", label: "Stop agent" }, { id: "restart", label: "Restart agent" }], (item) => item.label, false, states);
                if (selected === BACK) continue sandboxes;
                if (!selected) return;
                if (selected.id === "resume") break sandboxes;
                if (["stop", "restart"].includes(selected.id)) {
                  if (!(await ctx.ui.confirm("MagicMtn agent", `${selected.id === "stop" ? "Stop" : "Restart"} the agent in this sandbox? This may interrupt active work.`))) continue;
                  await magic(["pi", selected.id, workspace.id]);
                  ctx.ui.notify("Command recorded; the sandbox supervisor will acknowledge it on reconnect.", "info");
                  continue;
                }
                const confirmation = await select(ctx, `Delete sandbox ${workspace.id} and its workspace files?`,
                  [{ id: "cancel", label: "Cancel" }, { id: "terminate", label: "Terminate" }], (item) => item.label, false);
                if (confirmation === BACK || confirmation?.id === "cancel") continue;
                if (!confirmation) return;
                const result = await magic(["pi", "terminate", workspace.id]);
                ctx.ui.notify(`Sandbox ${workspace.id}: ${result.status}`, "info");
                workspace = undefined;
                continue sandboxes;
              }
            }
          } else if (!["new", "New workspace"].includes(action)) {
            ctx.ui.notify("Usage: /magic [default|view|new|inference]", "info");
            return;
          }
          if (!workspace) {
            catalog ??= await magic(["pi", "catalog"]);
            if (!catalog.default_image) throw new Error("MagicMtn Canonical must be published and enabled before creating a workspace.");
            const usePi = catalog.defaults.inference_source === "pi" && ctx.model && ctx.model.provider !== "magicmtn";
            const model = catalog.models.find((m) => m.id === catalog.defaults.model_id);
            if (!usePi && !model) throw new Error("No default inference model available. Run /magic default to configure one.");
            const inference = usePi
              ? ["--pi-connection", (await connectPiModel(ctx)).id]
              : ["--model", model.id];
            ctx.ui.notify(`Starting workspace with ${catalog.default_image.display_name}…`, "info");
            workspace = await magic(["pi", "create", ...inference]);
          }
          ctx.ui.notify(`Connecting workspace ${workspace.id}. Ctrl+] returns here; the sandbox stays running.`, "info");
          try {
            await terminal(ctx, ["pi", "attach", workspace.id]);
          } finally {
            ctx.ui.setStatus("magicmtn-workspace", `Local pi · disconnected from MagicMtn computer ${workspace.id.slice(0, 8)}`);
          }
          return;
        } catch (error) {
          const operation = error.attachFailure ? "attach"
            : ["new", "New workspace"].includes(action) ? "create"
            : ["open", "view"].includes(action) ? "resume"
            : action === "restart" ? "restart" : undefined;
          const expected = /cancelled|must be published and enabled|No default inference model/i.test(error.message);
          if (mode !== "local" && operation && !error.reported && !expected) {
            await magic(["pi", "report-error", "--operation", operation]).catch(() => {});
          }
          ctx.ui.notify(error.message, "error");
          return;
        }
      }
    },
  });
}
