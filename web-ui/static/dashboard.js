(() => {
  const root = document.documentElement;
  const dateOptions = {year: "numeric", month: "short", day: "numeric"};
  const localFormats = {
    date: new Intl.DateTimeFormat(undefined, dateOptions),
    minute: new Intl.DateTimeFormat(undefined, {...dateOptions, hour: "numeric", minute: "2-digit", timeZoneName: "short"}),
    second: new Intl.DateTimeFormat(undefined, {...dateOptions, hour: "numeric", minute: "2-digit", second: "2-digit", timeZoneName: "short"}),
  };
  function renderLocalTimes(container) {
    const times = container.querySelectorAll("time[data-local-time]");
    for (const time of times) {
      const date = new Date(time.dateTime);
      const format = localFormats[time.dataset.localTime];
      if (format && Number.isFinite(date.getTime())) time.textContent = format.format(date);
    }
  }
  renderLocalTimes(document);
  document.addEventListener("htmx:afterSwap", (event) => renderLocalTimes(event.target));
  const section = location.pathname.split("/")[1];
  document.querySelectorAll('.sidebar-workspace > a, .sidebar-manage > a, .sidebar-resources > a').forEach((link) => {
    const href = link.getAttribute("href");
    if (href === `/${section}` || href === location.pathname || link.dataset.activeSection === section) {
      link.setAttribute("aria-current", "page");
    }
  });
  document.querySelectorAll(".account-menu-panel a").forEach((link) => {
    if (link.getAttribute("href") === location.pathname) link.setAttribute("aria-current", "page");
  });
  document.querySelectorAll(".account-menu").forEach((accountMenu) => {
    if (!(accountMenu instanceof HTMLDetailsElement)) return;
    accountMenu.addEventListener("toggle", () => {
      if (accountMenu.open) document.querySelectorAll(".account-menu").forEach((other) => {
        if (other !== accountMenu) other.open = false;
      });
    });
    document.addEventListener("click", (event) => {
      if (!accountMenu.contains(event.target)) accountMenu.open = false;
    });
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && accountMenu.open) {
        accountMenu.open = false;
        accountMenu.querySelector("summary").focus();
      }
    });
    accountMenu.addEventListener("focusout", (event) => {
      if (event.relatedTarget && !accountMenu.contains(event.relatedTarget)) accountMenu.open = false;
    });
  });
  const pageTitle = document.getElementById("workspace-page-title");
  if (pageTitle && !pageTitle.textContent) pageTitle.textContent = document.title.split(" · ")[0] || "MagicMtn";

  const sidebar = document.getElementById("workspace-sidebar");
  const sidebarToggle = document.getElementById("sidebar-toggle");
  const backdrop = document.querySelector(".sidebar-backdrop");
  const frame = document.getElementById("workspace-frame");
  const statusbar = document.querySelector(".dashboard-statusbar");
  const mobileViewport = window.matchMedia?.("(max-width: 860px)");
  function setSidebar(open) {
    if (!sidebar || !sidebarToggle || !backdrop) return;
    document.body.classList.toggle("sidebar-open", open);
    sidebarToggle.setAttribute("aria-expanded", String(open));
    backdrop.hidden = !open;
    if (frame) frame.inert = open;
    if (statusbar) statusbar.inert = open;
    if (open) sidebar.querySelector("[data-sidebar-close]")?.focus();
    else sidebarToggle.focus({ preventScroll: true });
  }
  const divider = document.getElementById("sidebar-divider");
  const sidebarPreference = "magicmtn-dashboard-sidebar";
  const minSidebarWidth = 216;
  const maxSidebarWidth = 360;
  const collapseThreshold = 160;
  let sidebarWidth = parseFloat(getComputedStyle(root).getPropertyValue("--sidebar-width")) || 240;
  let sidebarCollapsed = false;
  let sidebarDrag = null;
  let sidebarSettleTimer;
  try {
    const saved = JSON.parse(localStorage.getItem(sidebarPreference));
    if (Number.isFinite(saved?.width)) sidebarWidth = Math.max(minSidebarWidth, Math.min(maxSidebarWidth, saved.width));
    sidebarCollapsed = saved?.collapsed === true;
  } catch { /* Storage may be unavailable or contain an old preference. */ }
  function renderDesktopSidebar() {
    if (!sidebar || !divider) return;
    // Animate the snap while keeping ordinary resizing directly under the pointer.
    if (sidebarDrag && document.body.classList.contains("sidebar-collapsed") !== sidebarCollapsed) {
      clearTimeout(sidebarSettleTimer);
      document.body.classList.add("sidebar-settling");
      sidebarSettleTimer = setTimeout(() => document.body.classList.remove("sidebar-settling"), 240);
    }
    document.body.style.setProperty("--sidebar-width", `${sidebarWidth}px`);
    document.body.classList.toggle("sidebar-collapsed", sidebarCollapsed);
    sidebar.inert = !mobileViewport?.matches && sidebarCollapsed;
    divider.setAttribute("aria-valuenow", String(sidebarCollapsed ? 0 : sidebarWidth));
    divider.setAttribute("aria-valuetext", sidebarCollapsed ? "Collapsed" : `${sidebarWidth} pixels`);
    if (!mobileViewport?.matches) {
      sidebarToggle?.setAttribute("aria-expanded", String(!sidebarCollapsed));
      sidebarToggle?.setAttribute("aria-label", sidebarCollapsed ? "Open navigation" : "Collapse navigation");
    }
  }
  function saveSidebar() {
    try { localStorage.setItem(sidebarPreference, JSON.stringify({ width: sidebarWidth, collapsed: sidebarCollapsed })); }
    catch { /* Keep the choice for this page. */ }
  }
  function resizeSidebar(width) {
    sidebarCollapsed = width < collapseThreshold;
    if (!sidebarCollapsed) sidebarWidth = Math.round(Math.max(minSidebarWidth, Math.min(maxSidebarWidth, width)));
    renderDesktopSidebar();
  }
  function finishSidebarDrag(cancel = false) {
    if (!sidebarDrag) return;
    const drag = sidebarDrag;
    sidebarDrag = null;
    document.body.classList.remove("sidebar-dragging");
    if (cancel) {
      sidebarWidth = drag.width;
      sidebarCollapsed = drag.collapsed;
      renderDesktopSidebar();
    } else saveSidebar();
    if (divider.hasPointerCapture(drag.pointerId)) divider.releasePointerCapture(drag.pointerId);
  }
  divider?.addEventListener("pointerdown", event => {
    if (mobileViewport?.matches || event.button !== 0 || sidebarDrag) return;
    event.preventDefault();
    sidebarDrag = { pointerId: event.pointerId, x: event.clientX, width: sidebarWidth, collapsed: sidebarCollapsed };
    divider.setPointerCapture(event.pointerId);
    clearTimeout(sidebarSettleTimer);
    document.body.classList.remove("sidebar-settling");
    document.body.classList.add("sidebar-dragging");
  });
  divider?.addEventListener("pointermove", event => {
    if (!sidebarDrag || event.pointerId !== sidebarDrag.pointerId) return;
    resizeSidebar((sidebarDrag.collapsed ? 12 : sidebarDrag.width) + event.clientX - sidebarDrag.x);
  });
  divider?.addEventListener("pointerup", event => {
    if (event.pointerId === sidebarDrag?.pointerId) finishSidebarDrag();
  });
  divider?.addEventListener("pointercancel", () => finishSidebarDrag(true));
  divider?.addEventListener("lostpointercapture", () => finishSidebarDrag(true));
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && sidebarDrag) {
      event.preventDefault();
      finishSidebarDrag(true);
    }
  });
  divider?.addEventListener("keydown", event => {
    if (mobileViewport?.matches || sidebarDrag) return;
    if (event.key === "Enter") sidebarCollapsed = !sidebarCollapsed;
    else if (event.key === "Home") sidebarCollapsed = true;
    else if (event.key === "End") resizeSidebar(maxSidebarWidth);
    else if (event.key === "ArrowLeft") {
      if (sidebarWidth <= minSidebarWidth) sidebarCollapsed = true;
      else if (!sidebarCollapsed) resizeSidebar(sidebarWidth - 16);
    } else if (event.key === "ArrowRight") {
      if (sidebarCollapsed) sidebarCollapsed = false;
      else resizeSidebar(sidebarWidth + 16);
    } else return;
    event.preventDefault();
    renderDesktopSidebar();
    saveSidebar();
  });
  renderDesktopSidebar();
  sidebarToggle?.addEventListener("click", () => {
    if (mobileViewport?.matches) setSidebar(true);
    else {
      sidebarCollapsed = !sidebarCollapsed;
      renderDesktopSidebar();
      saveSidebar();
      if (!sidebarCollapsed) sidebar?.querySelector('[aria-current="page"]')?.focus();
    }
  });
  document.querySelectorAll("[data-sidebar-close]").forEach(button => button.addEventListener("click", () => setSidebar(false)));
  sidebar?.addEventListener("click", event => {
    if (event.target instanceof Element && event.target.closest("a[href]") && document.body.classList.contains("sidebar-open")) setSidebar(false);
  });
  document.addEventListener("keydown", event => {
    if (!document.body.classList.contains("sidebar-open")) return;
    if (event.key === "Escape") {
      event.preventDefault();
      setSidebar(false);
    } else if (event.key === "Tab") {
      const items = [...sidebar.querySelectorAll('a[href], button, summary')].filter(element =>
        !element.closest("[hidden]") && !element.disabled &&
        (!element.closest(".account-menu-panel") || element.closest("details").open));
      const first = items[0], last = items.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  });
  mobileViewport?.addEventListener("change", event => {
    finishSidebarDrag(true);
    if (!event.matches && document.body.classList.contains("sidebar-open")) {
      setSidebar(false);
      if (!sidebarCollapsed) sidebar?.querySelector('[aria-current="page"]')?.focus();
    }
    renderDesktopSidebar();
    if (event.matches) {
      sidebarToggle?.setAttribute("aria-label", "Open navigation");
      sidebarToggle?.setAttribute("aria-expanded", "false");
    }
  });
  const connectionStatus = document.getElementById("connection-status");
  function updateConnection() {
    if (connectionStatus) connectionStatus.hidden = navigator.onLine !== false;
  }
  window.addEventListener("online", updateConnection);
  window.addEventListener("offline", updateConnection);
  updateConnection();

  const shortcuts = document.getElementById("keyboard-shortcuts");
  if (!(shortcuts instanceof HTMLDialogElement)) return;

  const themePreference = document.getElementById("theme-preference");
  const themeButton = document.getElementById("statusbar-theme");
  function updateThemeLabel() {
    const dark = root.dataset.theme !== "light";
    const label = document.getElementById("statusbar-theme-label");
    if (label) label.textContent = dark ? "Dark mode" : "Light mode";
    themeButton?.setAttribute("aria-label", dark ? "Switch to light mode" : "Switch to dark mode");
  }
  function setTheme(theme) {
    root.dataset.theme = theme;
    updateThemeLabel();
    if (themePreference instanceof HTMLSelectElement) themePreference.value = theme;
    try { localStorage.setItem("magicmtn-dashboard-theme", theme); } catch { /* Keep the choice for this page. */ }
  }
  if (themePreference instanceof HTMLSelectElement) {
    themePreference.value = root.dataset.theme === "light" ? "light" : "dark";
    themePreference.addEventListener("change", () => setTheme(themePreference.value));
  }
  function toggleTheme() {
    setTheme(root.dataset.theme === "light" ? "dark" : "light");
  }

  themeButton?.addEventListener("click", toggleTheme);
  updateThemeLabel();

  function revealSidebar(anchor) {
    if (!sidebar?.contains(anchor)) return () => {};
    if (mobileViewport?.matches && !document.body.classList.contains("sidebar-open")) {
      setSidebar(true);
      return () => setSidebar(false);
    }
    if (!mobileViewport?.matches && sidebarCollapsed) {
      sidebarCollapsed = false;
      renderDesktopSidebar();
      return () => {
        sidebarCollapsed = true;
        renderDesktopSidebar();
      };
    }
    return () => {};
  }
  document.dispatchEvent(new CustomEvent("magicmtn:dashboard-ready", { detail: { revealSidebar } }));
  const goDestinations = new Map();
  for (const link of document.querySelectorAll("a[data-go-key]")) {
    const key = link.dataset.goKey;
    goDestinations.set(key, link.getAttribute("href"));
    const row = document.createElement("div");
    row.className = "shortcut-row";
    const label = document.createElement("span");
    label.textContent = "Go to " + link.textContent.trim();
    const sequence = document.createElement("span");
    sequence.className = "shortcut-sequence";
    sequence.innerHTML = "<kbd>g</kbd><span>then</span><kbd></kbd>";
    sequence.lastElementChild.textContent = key;
    row.append(label, sequence);
    document.getElementById("navigation-shortcuts")?.append(row);
  }
  let goUntil = 0;
  document.addEventListener("keydown", (event) => {
    if (event.repeat || event.isComposing || event.altKey || event.ctrlKey || event.metaKey) {
      goUntil = 0;
      return;
    }
    const target = event.target;
    if (target instanceof Element && (target.closest("input, textarea, select, [contenteditable]") || target.isContentEditable)) {
      goUntil = 0;
      return;
    }

    const key = event.key.toLowerCase();
    const destination = goDestinations.get(key);
    if (destination && Date.now() < goUntil) {
      goUntil = 0;
      event.preventDefault();
      location.assign(destination);
      return;
    }
    goUntil = key === "g" ? Date.now() + 1500 : 0;

    if (event.key === "/") {
      event.preventDefault();
      if (shortcuts.open) shortcuts.close();
      else shortcuts.showModal();
    } else if (key === "d") {
      event.preventDefault();
      toggleTheme();
    }
  });
})();

document.addEventListener("click", (event) => {
  if (!(event.target instanceof Element)) return;
  const trigger = event.target.closest("[data-ck-modal-open]");
  if (!trigger) return;

  const dialog = document.getElementById(trigger.dataset.ckModalOpen);
  if (dialog instanceof HTMLDialogElement && !dialog.open) dialog.showModal();
});
