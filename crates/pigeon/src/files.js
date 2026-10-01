// The Files page's tree: folders open and close in place, the open ones
// are remembered per group for the session and opened again each time
// live.js swaps the page, and ?under= opens the tree down to a folder and
// scrolls to it. Each row's ⋯ opens a menu of what can be done to it; each
// choice opens its dialog, filled for that row, which says when a change
// becomes a request to the owner.
"use strict";
(() => {
  const table = () => document.querySelector("table.tree");
  const key = () => `pigeon-open:${table().dataset.group}`;

  const ancestors = (path) => {
    const names = path.split("/");
    return names.slice(0, -1).map((_, index) => names.slice(0, index + 1).join("/"));
  };

  const folders = () => [...table().querySelectorAll("tr[data-folder]")];
  const shownOpen = () =>
    new Set(folders().filter((row) => row.dataset.open !== undefined).map((row) => row.dataset.path));

  const remembered = () => {
    const saved = sessionStorage.getItem(key());
    return saved === null ? null : new Set(JSON.parse(saved));
  };

  const save = (open) => sessionStorage.setItem(key(), JSON.stringify([...open]));

  const apply = (open) => {
    for (const row of table().querySelectorAll("tr[data-path]")) {
      const path = row.dataset.path;
      if (row.dataset.folder !== undefined) {
        row.toggleAttribute("data-open", open.has(path));
        row.querySelector(".arrow").textContent = open.has(path) ? "▾" : "▸";
      }
      row.hidden = ancestors(path).some((folder) => !open.has(folder));
    }
  };

  const setUp = (first) => {
    if (table() === null) return;
    const open = remembered() ?? shownOpen();
    const under = table().dataset.under;
    if (first && under) {
      for (const folder of [...ancestors(under), under]) open.add(folder);
      save(open);
    }
    apply(open);
    if (!first || !under) return;
    table().querySelector("tr.target")?.scrollIntoView({ block: "center" });
  };

  const change = (open) => {
    apply(open);
    save(open);
  };

  let row = null;

  const fill = (dialog) => {
    const { kind, path, pattern, writable } = row.dataset;
    for (const subject of dialog.querySelectorAll(".subject")) {
      subject.textContent = path === "" ? table().dataset.group : path + (kind === "file" ? "" : "/");
    }
    const free = dialog.id === "add" ? row.dataset.addable : writable;
    for (const request of dialog.querySelectorAll(".request")) request.hidden = free !== "false";
    const set = (name, value) => {
      for (const field of dialog.querySelectorAll(`[name=${name}]`)) field.value = value;
    };
    set("back", location.pathname + location.search);
    set("path", dialog.id === "add" ? (path === "" ? "" : `${path}/`) : path);
    set("from", path);
    set("to", path);
    set("pattern", pattern ?? "");
    set("message", "");
    for (const file of dialog.querySelectorAll("input[type=file]")) file.value = "";
  };

  const showMenu = () => {
    const menu = document.getElementById("menu");
    const { kind, waiting, freezes, published, path } = row.dataset;
    const file = kind === "file" && published === "true";
    const draft = kind === "file" && published === "false";
    const folder = kind === "folder";
    const offered = {
      rename: file || draft || folder,
      replace: file || draft,
      add: folder || kind === "root",
      delete: file || draft || folder,
    };
    for (const choice of menu.querySelectorAll("[data-open]")) {
      choice.hidden = !offered[choice.dataset.open];
    }
    menu.querySelector("#download").hidden = !(file || folder);
    const publish = menu.querySelector("#publish");
    publish.hidden = Number(waiting) === 0;
    if (freezes === "true") {
      publish.dataset.confirm = `Publishing freezes what waits in a drop folder at ${path || table().dataset.group}: it then changes only through requests. Publish now?`;
    } else {
      delete publish.dataset.confirm;
    }
    const history = menu.querySelector("#history");
    history.hidden = !file;
    history.href = `/g/${encodeURIComponent(table().dataset.group)}/file?path=${encodeURIComponent(path)}`;
    fill(menu);
    menu.showModal();
  };

  document.addEventListener("click", (event) => {
    if (table() === null) return;
    const target = event.target;
    if (!(target instanceof Element)) return;
    const twist = target.closest("button.twist");
    if (twist !== null) {
      const open = shownOpen();
      const path = twist.closest("tr").dataset.path;
      if (open.has(path)) open.delete(path);
      else open.add(path);
      change(open);
    } else if (target.closest("#expand-all") !== null) {
      change(new Set(folders().map((folder) => folder.dataset.path)));
    } else if (target.closest("#collapse-all") !== null) {
      change(new Set());
    } else if (target.closest("button.more") !== null) {
      row = target.closest("button.more");
      showMenu();
    } else if (target.closest("[data-open]") !== null) {
      const dialog = document.getElementById(target.closest("[data-open]").dataset.open);
      document.getElementById("menu").close();
      fill(dialog);
      dialog.showModal();
    } else if (target.closest("button.close") !== null) {
      target.closest("dialog").close();
    }
  });

  document.addEventListener("mainswap", () => setUp(false));
  setUp(true);
})();
