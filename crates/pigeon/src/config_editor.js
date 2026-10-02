// The editor of a group's config.toml on its Overview page: the text,
// highlighted as TOML by a layer drawn under it, which the daemon reads
// after each keystroke to preview what saving would download, free and
// pin, rule by rule; a pin's row offers the times its files have
// versions at, the present or any local time, and choosing one writes it
// into its line, as does a pin line that misses its time, under the error
// it gets. Save applies the whole text, asking first when it frees space,
// and refusing when the file changed elsewhere since the editor loaded it.
"use strict";
(() => {
  const editor = document.getElementById("config");
  const group = document.body.dataset.group;
  const text = editor.querySelector("textarea");
  const layer = editor.querySelector(".code pre");
  const problem = editor.querySelector(".problem");
  const fix = editor.querySelector(".fix");
  const save = editor.querySelector("button.save");
  const conflict = editor.querySelector(".conflict");
  const panel = editor.querySelector(".preview");
  let base = editor.dataset.version;
  let latest = base;
  let question = null;
  let timer = null;
  let asked = 0;

  const escape = (raw) =>
    raw.replace(/[&<>]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[character]);

  const span = (kind, raw) => `<span class="t-${kind}">${escape(raw)}</span>`;

  const string = (raw) => {
    const rule = /^(["'])(follow|pin|free)(?=\s)/.exec(raw);
    if (rule === null) return span("string", raw);
    const rest = raw.slice(rule[0].length);
    return `<span class="t-string">${escape(rule[1])}${span("mode", rule[2])}${escape(rest)}</span>`;
  };

  const tokens = /("(?:[^"\\]|\\.)*"?|'[^']*'?)|(#.*$)|(\b(?:true|false)\b|[+-]?\b\d[\d_.:TZ+-]*)/g;

  const values = (raw) => {
    let html = "";
    let at = 0;
    for (const found of raw.matchAll(tokens)) {
      html += escape(raw.slice(at, found.index));
      const [whole, quoted, comment] = found;
      if (quoted !== undefined) html += string(whole);
      else if (comment !== undefined) html += span("comment", whole);
      else html += span("literal", whole);
      at = found.index + whole.length;
    }
    return html + escape(raw.slice(at));
  };

  const line = (raw) => {
    if (/^\s*#/.test(raw)) return span("comment", raw);
    const table = /^(\s*)(\[\[?[^\]]*\]\]?)(.*)$/.exec(raw);
    if (table !== null) return escape(table[1]) + span("table", table[2]) + values(table[3]);
    const key = /^(\s*)([\w.\-"]+)(\s*=)(.*)$/.exec(raw);
    if (key !== null) return escape(key[1]) + span("key", key[2]) + escape(key[3]) + values(key[4]);
    return values(raw);
  };

  const draw = () => {
    layer.innerHTML = text.value.split("\n").map(line).join("\n") + "\n ";
    text.style.height = "auto";
    text.style.height = `${text.scrollHeight + 2}px`;
  };

  const toDate = (time) => new Date(time.replace(/(\.\d{3})\d+/, "$1"));

  const toLocal = (time) => {
    const date = toDate(time);
    if (Number.isNaN(date.getTime())) return "";
    const shifted = new Date(date.getTime() - date.getTimezoneOffset() * 60000);
    return shifted.toISOString().slice(0, 19);
  };

  const label = () => {
    for (const option of editor.querySelectorAll("option[data-time]")) {
      const date = toDate(option.dataset.time);
      if (Number.isNaN(date.getTime())) continue;
      const files = option.dataset.files;
      option.textContent = date.toLocaleString() + (files === undefined ? "" : ` · ${files}`);
    }
    for (const field of editor.querySelectorAll("input.pin-time")) {
      field.value = toLocal(field.dataset.time);
    }
  };

  const show = (parts) => {
    latest = parts.version;
    conflict.hidden = latest === base;
    problem.hidden = true;
    fix.replaceChildren();
    panel.innerHTML = parts.panel;
    panel.style.opacity = "";
    label();
    save.textContent = parts.save;
    save.disabled = false;
    question = parts.confirm;
  };

  const refuse = (error, pins) => {
    problem.textContent = error;
    problem.hidden = false;
    fix.innerHTML = pins ?? "";
    label();
    panel.style.opacity = "0.5";
    save.textContent = "Save";
    save.disabled = true;
  };

  const preview = async () => {
    clearTimeout(timer);
    const id = ++asked;
    const body = new URLSearchParams({ text: text.value });
    try {
      const response = await fetch(`/g/${encodeURIComponent(group)}/config/preview`, {
        method: "POST",
        body,
      });
      const parts = await response.json();
      if (id !== asked) return false;
      if (parts.error !== undefined) {
        refuse(parts.error, parts.fix);
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

  const pin = (row, time) => {
    const start = Number(row.dataset.start);
    const end = Number(row.dataset.end);
    const spelled = text.value.slice(start, end);
    if (!/^["'].*["']$/s.test(spelled)) {
      preview();
      return;
    }
    const line = `pin ${time} ${row.dataset.pattern}`.trimEnd();
    text.setRangeText(JSON.stringify(line), start, end, "preserve");
    draw();
    preview();
  };

  editor.addEventListener("change", (event) => {
    const field = event.target;
    const row = field.closest("[data-start]");
    if (row === null) return;
    if (field.matches("select.pin")) {
      pin(row, field.value === "now" ? new Date().toISOString() : field.value);
    } else if (field.matches("input.pin-time") && field.value !== "") {
      pin(row, new Date(field.value).toISOString().replace(".000Z", "Z"));
    }
  });

  text.addEventListener("input", () => {
    draw();
    schedule();
  });

  conflict.querySelector(".load").addEventListener("click", () => location.reload());
  conflict.querySelector(".overwrite").addEventListener("click", () => {
    base = latest;
    conflict.hidden = true;
  });

  save.addEventListener("click", async () => {
    if (!(await preview())) return;
    if (question !== null && !confirm(question)) return;
    const body = new FormData();
    body.set("group", group);
    body.set("text", new Blob([text.value], { type: "text/plain" }));
    body.set("version", base);
    if (question !== null) body.set("yes", "true");
    body.set("back", location.pathname);
    save.disabled = true;
    const response = await fetch("/act/config/set", { method: "POST", body });
    if (response.ok) {
      location.reload();
      return;
    }
    const page = new DOMParser().parseFromString(await response.text(), "text/html");
    alert(page.querySelector(".error")?.textContent ?? response.statusText);
    preview();
  });

  document.addEventListener("groupchange", schedule);
  draw();
  preview();
})();
