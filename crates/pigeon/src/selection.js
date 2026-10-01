// The selection editor: rows of rules held as a draft, which the daemon
// previews after each change and as files arrive, saved whole only by Save,
// which asks first when it frees space and refuses when the selection
// changed elsewhere since the draft began.
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

  const line = (row) => {
    const mode = row.querySelector(".mode").value;
    const pattern = row.querySelector(".pattern").value;
    if (mode !== "frozen") return `${mode} ${pattern}`;
    const time = row.querySelector(".time").value.trim();
    const at = row.querySelector(".at").value === "now" ? "now" : time || "-";
    return `frozen ${at} ${pattern}`;
  };

  const draft = () => [...list.children].map(line).join("\n") + "\n";

  const shape = (row) => {
    const frozen = row.querySelector(".mode").value === "frozen";
    row.querySelector(".when").hidden = !frozen;
    row.querySelector(".time").hidden = row.querySelector(".at").value !== "time";
  };

  const add = (rule) => {
    const row = template.content.firstElementChild.cloneNode(true);
    row.querySelector(".mode").value = rule.mode;
    row.querySelector(".pattern").value = rule.pattern;
    if (rule.time) {
      row.querySelector(".at").value = "time";
      row.querySelector(".time").value = rule.time;
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
    shape(event.target.closest("li"));
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
