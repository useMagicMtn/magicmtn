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
  const call = async (name, args = {}) => {
    const result = await request(["--local", "mcp", "call", name], undefined, args);
    if (result.isError) throw new Error(result.content.map((item) => item.text || "").join("\n"));
    return JSON.parse(result.content[0].text);
  };
  if (!action) action = await ctx.ui.select("MagicMtn · Local", ["New sandbox", "View computers"]);
  if (!action) return;
  if (["default", "inference", "Inference only"].includes(action)) {
    ctx.ui.notify("Choose your Pi model with /model. Pi uses your existing provider login.", "info");
    return;
  }
  if (["new", "New sandbox"].includes(action)) {
    const image = await ctx.ui.input("Sandbox image", "python:3.13-alpine");
    if (image === undefined) return;
    const sandbox = await call("sandbox_create", { image: image || "python:3.13-alpine" });
    ctx.ui.notify(`Created sandbox ${sandbox.id}. Pi can use the MagicMtn sandbox tools.`, "info");
    return;
  }
  if (!["view", "open", "View computers"].includes(action)) {
    ctx.ui.notify("Usage: /magic [new|view|inference]", "info");
    return;
  }
  const sandboxes = (await call("sandbox_list")).filter((item) => ["running", "starting"].includes(item.status));
  if (!sandboxes.length) {
    ctx.ui.notify("Use /magic new to create a sandbox.", "info");
    return;
  }
  const selection = await ctx.ui.select("Local sandboxes", sandboxes.map((item) => `${item.id} · ${item.status}`));
  if (!selection) return;
  const sandbox = sandboxes.find((item) => selection.startsWith(item.id));
  if (!sandbox) return;
  const operation = await ctx.ui.select(`Sandbox ${sandbox.id}`, ["Open shell", "Terminate"]);
  if (operation === "Open shell") await terminal(ctx, ["--local", "computer", "ssh", sandbox.id]);
  if (operation === "Terminate" && await ctx.ui.confirm("Terminate sandbox", `Delete sandbox ${sandbox.id} and its files?`)) {
    await call("sandbox_terminate", { id: sandbox.id });
    ctx.ui.notify(`Terminated sandbox ${sandbox.id}.`, "info");
  }
}
