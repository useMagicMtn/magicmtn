(() => {
  const addPanel = document.getElementById("mcp-add-panel");
  const connectedPanel = document.getElementById("mcp-connected-panel");
  const list = document.getElementById("mcp-list");
  const search = document.getElementById("mcp-search-input");
  const searchStatus = document.getElementById("mcp-search-status");
  const catalog = new Map([...document.querySelectorAll("#mcp-catalog article")].map(card => {
    const provider = card.id.slice("mcp-add-".length);
    const text = [...card.querySelectorAll(".integration-category, h3, p")]
      .map(field => field.textContent).join(" ").toLowerCase();
    return [provider, {card, text, available: true}];
  }));

  function filterCatalog() {
    const terms = search.value.toLowerCase().trim().split(/\s+/).filter(Boolean);
    const matches = text => terms.every(term => text.includes(term));
    let availableMatches = 0;
    let connectedMatches = 0;
    for (const {card, text, available} of catalog.values()) {
      card.hidden = !available || !matches(text);
      if (!card.hidden) availableMatches++;
    }
    for (const card of list?.children ?? []) {
      card.hidden = !matches(card.dataset.search);
      if (!card.hidden) connectedMatches++;
    }
    const searching = terms.length > 0;
    addPanel.hidden = searching && availableMatches === 0;
    if (connectedPanel) connectedPanel.hidden = searching && connectedMatches === 0;
    const count = availableMatches + connectedMatches;
    searchStatus.textContent = !searching ? "" : count === 0 ? "No MCPs match your search." : `${count} ${count === 1 ? "result" : "results"}`;
  }

  search.addEventListener("input", filterCatalog);
  document.getElementById("mcp-search").hidden = false;
  filterCatalog();
  window.magicMtnMcpCatalog = { catalog, filterCatalog };
})();
