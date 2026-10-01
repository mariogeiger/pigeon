// The selection editor: rows of rules held as a draft, which the daemon
// previews after each change and as files arrive, saved whole only by Save,
// which asks first when it frees space and refuses when the selection
// changed elsewhere since the draft began. A pin holds its files at now, at
// one of the versions the preview lists, or at a time picked in local time;
// each time is kept exactly as the RFC 3339 text the daemon reads.
"use strict";
(() => {
  const editor = document.getElementById("editor");
  const group = document.body.dataset.group;
  const list = editor.querySelector("ol.rules");
  const template = editor.querySelector("template");
  const save = editor.querySelector("button.save");
  const conflict = editor.querySelector(".conflict");
  const panel = editor.querySelector(".preview");
  let base = editor.dataset.version;
  let latest = base;
  let question = null;
  let timer = null;
  let asked = 0;

  const toDate = (time) => new Date(time.replace(/(\.\d{3})\d+/, "$1"));

  const toLocal = (time) => {
    const date = toDate(time);
    const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60000);
    return shifted.toISOString().slice(0, 19);
  };

  const pinned = (row) => {
    const at = row.querySelector(".at").value;
    if (at !== "time") return at;
    return row.dataset.time || "-";
  };

  const line = (row) => {
    const mode = row.querySelector(".mode").value;
    const pattern = row.querySelector(".pattern").value;
    if (mode !== "pin") return `${mode} ${pattern}`;
    return `pin ${pinned(row)} ${pattern}`;
  };

  const draft = () => [...list.children].map(line).join("\n") + "\n";

  const shape = (row) => {
    const pin = row.querySelector(".mode").value === "pin";
    row.querySelector(".when").hidden = !pin;
    row.querySelector(".time").hidden = row.querySelector(".at").value !== "time";
  };

  const offer = (row, times) => {
    const versions = row.querySelector(".versions");
    const known = JSON.stringify(times);
    if (versions.dataset.times === known) return;
    versions.dataset.times = known;
    const at = row.querySelector(".at");
    const chosen = at.value === "time" ? row.dataset.time : at.value;
    versions.replaceChildren(
      ...times.map(({ time, files }) => {
        const label = `${toDate(time).toLocaleString()} · ${files} file${files === 1 ? "" : "s"}`;
        return new Option(label, time);
      }),
    );
    versions.hidden = times.length === 0;
    if (times.some(({ time }) => time === chosen)) {
      at.value = chosen;
    } else if (chosen !== "now") {
      at.value = "time";
      if (chosen) row.dataset.time = chosen;
    }
    shape(row);
  };

  const add = (rule) => {
    const row = template.content.firstElementChild.cloneNode(true);
    row.querySelector(".mode").value = rule.mode;
    row.querySelector(".pattern").value = rule.pattern;
    if (rule.time) {
      row.dataset.time = rule.time;
      row.querySelector(".at").value = "time";
      row.querySelector(".time").value = toLocal(rule.time);
    }
    shape(row);
    list.append(row);
  };

  const show = (parts) => {
    latest = parts.version;
    conflict.hidden = latest === base;
    [...list.children].forEach((row, index) => {
      const part = parts.rows[index] ?? {};
      row.querySelector(".effect").textContent = part.effect ?? "";
      offer(row, part.times ?? []);
      row.classList.toggle("masked", part.masked === true);
      const error = row.querySelector(".error");
      error.hidden = part.error === undefined;
      error.textContent = part.error ?? "";
    });
    panel.innerHTML = parts.panel;
    save.textContent = parts.save;
    save.disabled = parts.rows.some((row) => row.error !== undefined);
    question = parts.confirm;
  };

  const preview = async () => {
    clearTimeout(timer);
    const id = ++asked;
    const body = new URLSearchParams({ rules: draft() });
    try {
      const response = await fetch(`/g/${encodeURIComponent(group)}/selection/preview`, {
        method: "POST",
        body,
      });
      const parts = await response.json();
      if (id !== asked) return false;
      if (parts.error !== undefined) {
        panel.textContent = parts.error;
        return false;
      }
      show(parts);
      return true;
    } catch {
      return false;
    }
  };

  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(preview, 150);
  };

  list.addEventListener("input", (event) => {
    const row = event.target.closest("li");
    const field = event.target;
    if (field.classList.contains("time")) {
      row.dataset.time = field.value === "" ? "" : new Date(field.value).toISOString();
    } else if (field.classList.contains("at") && field.value === "time" && row.dataset.time) {
      row.querySelector(".time").value = toLocal(row.dataset.time);
    } else if (field.classList.contains("at") && field.value !== "now" && field.value !== "time") {
      row.dataset.time = field.value;
    }
    shape(row);
    schedule();
  });

  list.addEventListener("click", (event) => {
    const button = event.target.closest("button");
    if (button === null) return;
    const row = button.closest("li");
    if (button.classList.contains("up") && row.previousElementSibling !== null) {
      row.previousElementSibling.before(row);
    } else if (button.classList.contains("down") && row.nextElementSibling !== null) {
      row.nextElementSibling.after(row);
    } else if (button.classList.contains("remove")) {
      row.remove();
    } else {
      return;
    }
    schedule();
  });

  editor.querySelector("form.add").addEventListener("submit", (event) => {
    event.preventDefault();
    const field = event.target.elements.pattern;
    const pattern = field.value.trim();
    if (pattern === "") return;
    add({ mode: "follow", pattern });
    field.value = "";
    schedule();
  });

  conflict.querySelector(".reload").addEventListener("click", () => location.reload());
  conflict.querySelector(".overwrite").addEventListener("click", () => {
    base = latest;
    conflict.hidden = true;
  });

  save.addEventListener("click", async () => {
    if (!(await preview()) || save.disabled) return;
    if (question !== null && !confirm(question)) return;
    const body = new FormData();
    body.set("group", group);
    body.set("rules", draft());
    body.set("version", base);
    body.set("back", location.pathname);
    save.disabled = true;
    const response = await fetch("/act/selection/set", { method: "POST", body });
    if (response.ok) {
      location.reload();
      return;
    }
    const page = new DOMParser().parseFromString(await response.text(), "text/html");
    alert(page.querySelector(".error")?.textContent ?? response.statusText);
    preview();
  });

  document.addEventListener("groupchange", schedule);
  for (const rule of JSON.parse(editor.dataset.rules)) add(rule);
  preview();
})();
