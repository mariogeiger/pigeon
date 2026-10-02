// Keeps a group's pages live: each event of the group's stream fetches the
// page again and swaps its bar and <main>, unless the person is typing,
// reading an open form or answering a question, keeping the parts marked
// data-keep, and tells the page through a groupchange event, and through
// a mainswap event once it swapped; follow boxes submit the page's form
// that follows their pattern, or ask first whether to pin an unfollowed
// copy here as it is now or to free it; forms marked data-confirm ask
// before they publish, and those inside data-in-place swap the page they
// return to in place; countdowns tick; once the daemon restarts onto
// another program, a banner offers to reload the page.
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
    const nav = page.querySelector("nav");
    if (nav !== null) document.querySelector("nav").replaceWith(nav);
    document.title = page.title;
    prepare(fresh);
    document.dispatchEvent(new Event("mainswap"));
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

  // Sends `form` and shows the page it returns to, or alerts its error.
  const send = async (form) => {
    const response = await fetch(form.action, { method: "POST", body: new FormData(form) });
    const text = await response.text();
    if (response.ok) {
      show(text);
      return;
    }
    const page = new DOMParser().parseFromString(text, "text/html");
    alert(page.querySelector(".error")?.textContent ?? response.statusText);
    refresh();
  };

  // The box whose unchecking the unfollow dialog asks about, checked
  // again unless one of its answers is sent.
  let unchecked = null;

  document.addEventListener("change", (event) => {
    const box = event.target;
    if (!(box instanceof HTMLInputElement) || box.dataset.pattern === undefined) return;
    for (const field of document.querySelectorAll("#follow [name=pattern], #unfollow [name=pattern]")) {
      field.value = box.dataset.pattern;
    }
    if (box.checked) {
      document.querySelector("#follow form").requestSubmit();
      return;
    }
    unchecked = box;
    document.getElementById("unfollow").showModal();
  });

  document.addEventListener("submit", (event) => {
    const form = event.target;
    const asking = form.closest("[data-confirm]");
    if (asking !== null && !confirm(asking.dataset.confirm)) {
      event.preventDefault();
      return;
    }
    if (form.closest("[data-in-place]") === null) return;
    event.preventDefault();
    unchecked = null;
    form.closest("dialog")?.close();
    send(form);
  });

  document.addEventListener(
    "close",
    (event) => {
      if (event.target.id === "unfollow" && unchecked !== null) {
        unchecked.checked = true;
        unchecked = null;
      }
      retry();
    },
    true,
  );

  document.addEventListener("focusout", retry);
  document.addEventListener("toggle", retry, true);
  setInterval(tick, 1000);
  prepare(document);

  const updated = document.getElementById("updated");
  updated.querySelector("button").addEventListener("click", () => location.reload());

  if (group !== undefined) {
    const events = new EventSource(`/g/${encodeURIComponent(group)}/events`);
    let program;
    events.addEventListener("program", (event) => {
      program ??= event.data;
      if (event.data !== program) updated.hidden = false;
    });
    events.onopen = refresh;
    events.onmessage = () => {
      document.dispatchEvent(new Event("groupchange"));
      refresh();
    };
  }
})();
