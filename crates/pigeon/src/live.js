// Keeps a group's pages live: each event of the group's stream fetches the
// page again and swaps its <main>, unless the person is typing, reading an
// open form or answering a question, keeping the parts marked data-keep,
// and tells the page through a groupchange event; follow boxes post the selection,
// asking first whether an unfollowed copy stays; countdowns tick; forms
// marked data-confirm ask before they publish.
"use strict";
(() => {
  const group = document.body.dataset.group;
  let loading = false;
  let stale = false;

  const busy = () => {
    const active = document.activeElement;
    const typing =
      active !== null &&
      active.closest("main") !== null &&
      active.matches("input:not([type=checkbox]), textarea, select");
    return typing || document.querySelector("dialog[open], main details[open]") !== null;
  };

  const tick = () => {
    for (const due of document.querySelectorAll("[data-at]")) {
      const left = Math.max(0, Math.round((Number(due.dataset.at) - Date.now()) / 1000));
      due.textContent = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`;
    }
  };

  const prepare = (root) => {
    for (const box of root.querySelectorAll("input[data-state=mixed]")) {
      box.indeterminate = true;
    }
    const now = Date.now();
    for (const due of root.querySelectorAll("[data-due]")) {
      due.dataset.at = String(now + Number(due.dataset.due) * 1000);
    }
    tick();
  };

  const show = (text) => {
    const page = new DOMParser().parseFromString(text, "text/html");
    const fresh = page.querySelector("main");
    if (fresh === null) return;
    for (const kept of fresh.querySelectorAll("[data-keep][id]")) {
      const current = document.getElementById(kept.id);
      if (current !== null) kept.replaceWith(current);
    }
    document.querySelector("main").replaceWith(fresh);
    document.title = page.title;
    prepare(fresh);
  };

  const refresh = async () => {
    if (loading || busy()) {
      stale = true;
      return;
    }
    loading = true;
    stale = false;
    try {
      const response = await fetch(location.href, { cache: "no-store" });
      const text = await response.text();
      if (busy()) stale = true;
      else if (response.ok) show(text);
    } catch {
      stale = true;
    } finally {
      loading = false;
    }
    if (stale && !busy()) refresh();
  };

  const retry = () => {
    if (stale) setTimeout(refresh, 0);
  };

  const post = async (verb, pattern, free) => {
    const body = new FormData();
    body.set("group", group);
    body.set("pattern", pattern);
    body.set("back", location.pathname + location.search);
    if (free) body.set("free", "true");
    const response = await fetch(`/act/selection/${verb}`, { method: "POST", body });
    const text = await response.text();
    if (response.ok) {
      show(text);
      return;
    }
    const page = new DOMParser().parseFromString(text, "text/html");
    alert(page.querySelector(".error")?.textContent ?? response.statusText);
    refresh();
  };

  const ask = (dialog) =>
    new Promise((resolve) => {
      dialog.returnValue = "";
      dialog.addEventListener("close", () => resolve(dialog.returnValue), { once: true });
      dialog.showModal();
    });

  document.addEventListener("change", async (event) => {
    const box = event.target;
    if (!(box instanceof HTMLInputElement) || box.dataset.pattern === undefined) return;
    if (box.checked) {
      await post("follow", box.dataset.pattern, false);
      return;
    }
    const answer = await ask(document.getElementById("unfollow"));
    if (answer === "") {
      box.checked = true;
      retry();
      return;
    }
    await post("unfollow", box.dataset.pattern, answer === "free");
  });

  document.addEventListener("submit", (event) => {
    const asking = event.target.closest("[data-confirm]");
    if (asking !== null && !confirm(asking.dataset.confirm)) event.preventDefault();
  });

  document.addEventListener("focusout", retry);
  document.addEventListener("toggle", retry, true);
  document.addEventListener("close", retry, true);
  setInterval(tick, 1000);
  prepare(document);

  if (group !== undefined) {
    const events = new EventSource(`/g/${encodeURIComponent(group)}/events`);
    events.onopen = refresh;
    events.onmessage = () => {
      document.dispatchEvent(new Event("groupchange"));
      refresh();
    };
  }
})();
