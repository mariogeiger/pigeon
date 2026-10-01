// The Files page's tree: folders open and close in place, the open ones
// are remembered per group for the session and opened again each time
// live.js swaps the page, and ?under= opens the tree down to a folder and
// scrolls to it. Each row's ⋯ opens a menu of what can be done to it, the
// changes waiting at it included; each choice opens its dialog, filled for
// that row, which asks only to confirm a change that is all the member's,
// and otherwise whether to apply it now or ask the owners first.
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
  let chosen = null;

  // Whether what `dialog` changes is all the member's.
  const mine = (dialog) => {
    if (dialog.id === "add") return row.dataset.addable !== "false";
    if (dialog.id === "place") return chosen.made && chosen.owned;
    if (dialog.id === "discard") return chosen.made;
    return row.dataset.writable !== "false";
  };

  const fill = (dialog) => {
    const { kind, path, pattern } = row.dataset;
    for (const subject of dialog.querySelectorAll(".subject")) {
      subject.textContent = path === "" ? table().dataset.group : path + (kind === "file" ? "" : "/");
    }
    const own = dialog.id === "menu" || mine(dialog);
    for (const part of dialog.querySelectorAll(".mine")) part.hidden = !own;
    for (const part of dialog.querySelectorAll(".theirs")) part.hidden = own;
    const set = (name, value) => {
      for (const field of dialog.querySelectorAll(`[name=${name}]`)) field.value = value;
    };
    set("back", location.pathname + location.search);
    set("path", dialog.id === "add" ? (path === "" ? "" : `${path}/`) : path);
    set("from", path);
    set("to", path);
    set("pattern", pattern ?? "");
    set("message", "");
    set("entry", chosen?.entry ?? "");
    for (const file of dialog.querySelectorAll("input[type=file]")) file.value = "";
  };

  // A button that posts `entry` to `change <verb>` at once.
  const post = (verb, entry, label) => {
    const form = document.createElement("form");
    form.method = "post";
    form.action = `/act/change/${verb}`;
    form.enctype = "multipart/form-data";
    const fields = { back: location.pathname + location.search, group: table().dataset.group, entry };
    for (const [name, value] of Object.entries(fields)) {
      const field = document.createElement("input");
      field.type = "hidden";
      field.name = name;
      field.value = value;
      form.append(field);
    }
    const button = document.createElement("button");
    button.textContent = label;
    form.append(button);
    return form;
  };

  // A button that opens the dialog `id` for `waiting`.
  const opener = (id, waiting, label) => {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => {
      chosen = waiting;
      const dialog = document.getElementById(id);
      document.getElementById("menu").close();
      fill(dialog);
      dialog.showModal();
    });
    return button;
  };

  // The lines of the changes waiting at the row, each with what resolves it.
  const waitingLines = () =>
    JSON.parse(row.dataset.changes ?? "[]").map((waiting) => {
      const line = document.createElement("div");
      line.className = "change";
      const what = document.createElement("p");
      what.textContent = `📬 ${waiting.title}`;
      line.append(what);
      if (waiting.waits === "applying") return line;
      line.append(post("apply", waiting.entry, "Apply"));
      if (waiting.waits === "aside" && !waiting.owned) {
        line.append(post("ask", waiting.entry, "Ask the owner"));
      }
      line.append(opener("place", waiting, "Place elsewhere…"));
      line.append(
        waiting.waits === "proposed"
          ? post("discard", waiting.entry, "Discard")
          : opener("discard", waiting, "Discard…"),
      );
      return line;
    });

  const showMenu = () => {
    const menu = document.getElementById("menu");
    const { kind, waiting, freezes, published, editable, path } = row.dataset;
    const file = kind === "file" && published === "true";
    const draft = kind === "file" && published === "false" && editable === "true";
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
    menu.querySelector(".waiting").replaceChildren(...waitingLines());
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
