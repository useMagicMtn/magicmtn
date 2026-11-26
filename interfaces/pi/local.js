// Pi keeps its model and host tools. Sandbox calls use the private local server.
export async function registerLocalTools(pi, request) {
  const definitions = await request(["--local", "mcp", "tools"]);
  const names = [];
  for (const definition of definitions) {
    const name = `magic_${definition.name}`;
    names.push(name);
    pi.registerTool({
      name,
      label: definition.name.replaceAll("_", " "),
      description: definition.description,
      parameters: definition.inputSchema,
      async execute(_id, args, signal) {
        return request(["--local", "mcp", "call", definition.name], signal, args);
      },
    });
  }
  return names;
}

export async function localMenu(action, ctx, request, terminal) {
  const run = (args) => request(["--local", "pi", ...args]);
  if (!action) action = await ctx.ui.select("MagicMtn · Local", ["New computer", "View computers", "Default model"]);
  if (!action) return;
  const chooseModel = async (catalog) => {
    const choices = catalog.models.map((model) => ({ ...model, label: `${model.display_name} · ${model.provider}` }));
    if (!choices.length) {
      ctx.ui.notify("Install and configure an inference provider with magic provider install and magic provider configure.", "info");
      return;
    }
    const selection = await ctx.ui.select("Default inference model", choices.map((model) => model.label));
    const model = choices.find((item) => item.label === selection);
    if (!model) return;
    ctx.ui.notify(`Preparing ${model.display_name} with ${model.provider}…`, "info");
    await run(["defaults", "--model", model.id]);
    ctx.ui.notify(`Default model: ${model.display_name} · ${model.provider}`, "info");
    return model.id;
  };
  if (["default", "inference", "Default model"].includes(action)) {
    await chooseModel(await run(["catalog"]));
    return;
  }
  if (["new", "New computer"].includes(action)) {
    const catalog = await run(["catalog"]);
    const modelId = catalog.defaults?.model_id || await chooseModel(catalog);
    if (!modelId) return;
    const images = await request(["--local", "images", "list", "--format", "json"]);
    const image = images.images.find((item) => item.is_default);
    const providers = [...new Set((image?.preparations || []).filter((item) => item.ready).map((item) => item.provider))];
    if (!providers.length) {
      ctx.ui.notify("Prepare Canonical with magic images prepare canonical --provider PROVIDER, then run /magic new.", "info");
      return;
    }
    const provider = providers.length === 1 ? providers[0] : await ctx.ui.select("Sandbox backend", providers);
    if (!provider) return;
    const workspace = await run(["local-create", "--provider", provider, "--model", modelId]);
    await terminal(ctx, ["--local", "pi", "attach", workspace.id]);
    return;
  }
  if (!["view", "open", "View computers"].includes(action)) {
    ctx.ui.notify("Usage: /magic [default|new|view]", "info");
    return;
  }
  const { workspaces } = await run(["list"]);
  const sessions = workspaces.filter((item) => item.desired !== "terminated");
  if (!sessions.length) {
    ctx.ui.notify("Use /magic new to create a computer.", "info");
    return;
  }
  const selection = await ctx.ui.select("Local computers", sessions.map((item) => `${item.id} · ${item.provider} · ${item.status}`));
  const workspace = sessions.find((item) => selection?.startsWith(item.id));
  if (!workspace) return;
  const operation = await ctx.ui.select(`Computer ${workspace.id}`, ["Resume", "Terminate"]);
  if (operation === "Resume") {
    const session = await run(["status", workspace.id]);
    if (["stopped", "exited"].includes(session.status)) await run(["restart", workspace.id]);
    await terminal(ctx, ["--local", "pi", "attach", workspace.id]);
  }
  if (operation === "Terminate" && await ctx.ui.confirm("Terminate computer", `Delete computer ${workspace.id} and its files?`)) {
    await run(["terminate", workspace.id]);
    ctx.ui.notify(`Terminated computer ${workspace.id}.`, "info");
  }
}
