(() => {
  const form = document.getElementById("template-form");
  const editor = document.getElementById("template-toml");
  const button = document.getElementById("template-save");
  const status = document.getElementById("template-status");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    button.disabled = true;
    editor.readOnly = true;
    status.textContent = "Saving…";
    status.dataset.error = "false";
    try {
      const response = await fetch("/api/templates/default", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ toml: editor.value }),
      });
      if (!response.ok) throw new Error(await response.text() || "Could not save template.");
      const saved = await response.json();
      editor.value = saved.toml;
      status.textContent = "Template saved.";
    } catch (error) {
      status.dataset.error = "true";
      status.textContent = error.message || "Could not save template. Try again.";
    } finally {
      button.disabled = false;
      editor.readOnly = false;
    }
  });
})();
