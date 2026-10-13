// MailBackup window logic. Plain JS, no build step, no external resources.
// Every piece of text that comes from the server, the disk or an error goes into the DOM through
// textContent / attributes (never innerHTML), so folder names, e-mail addresses or error texts
// can never inject markup.
"use strict";

(() => {
  const T = window.__TAURI__;
  const invoke = (cmd, args) => T.core.invoke(cmd, args);
  const listen = (ev, cb) => T.event.listen(ev, cb);

  // ---------- tiny DOM helpers ----------
  const $ = (id) => document.getElementById(id);
  function h(tag, attrs, ...children) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === undefined || v === null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "text") el.textContent = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : String(v));
    }
    for (const c of children.flat()) {
      if (c === null || c === undefined || c === false) continue;
      el.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return el;
  }
  const clear = (el) => { while (el.firstChild) el.removeChild(el.firstChild); };
  const errText = (e) => (typeof e === "string" ? e : (e && e.message) || String(e));
  const nf = new Intl.NumberFormat("pt-BR");
  const num = (n) => (n === null || n === undefined ? "—" : nf.format(n));
  const dtf = new Intl.DateTimeFormat("pt-BR", { dateStyle: "medium", timeStyle: "short" });
  function when(secs) {
    if (!secs) return "Nunca";
    const d = new Date(secs * 1000);
    const diff = (Date.now() - d.getTime()) / 1000;
    let rel = "";
    if (diff < 60) rel = "agora mesmo";
    else if (diff < 3600) rel = `há ${Math.round(diff / 60)} min`;
    else if (diff < 86400) rel = `há ${Math.round(diff / 3600)} h`;
    else if (diff < 86400 * 30) rel = `há ${Math.round(diff / 86400)} dia(s)`;
    return rel ? `${dtf.format(d)} (${rel})` : dtf.format(d);
  }
  const OPS = { archive: "Arquivamento", export: "Exportação", migrate: "Migração" };

  let toastTimer = 0;
  function toast(msg) {
    const t = $("toast");
    t.textContent = msg;
    t.hidden = false;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { t.hidden = true; }, 3500);
  }

  function setBusy(btn, busy, label) {
    if (busy) {
      btn.dataset.label = btn.textContent;
      btn.disabled = true;
      if (label) btn.textContent = label;
    } else {
      btn.disabled = false;
      if (btn.dataset.label !== undefined && label) btn.textContent = btn.dataset.label;
    }
  }

  // ---------- state ----------
  const state = {
    profiles: [],
    selected: null,
    status: {},     // name -> StatusReport | {error}
    live: {},       // name -> { progress, failures: [] }
    results: {},    // name -> { kind, title, text, failures, total }
  };
  const current = () => state.profiles.find((p) => p.name === state.selected) || null;

  // ---------- sidebar ----------
  function dotClass(p) {
    if (p.running || state.live[p.name]) return "running";
    if (!p.lastRun) return "";
    return { ok: "ok", partial: "partial", error: "error", cancelled: "" }[p.lastRun.class] || "";
  }
  function dotLabel(p) {
    if (p.running || state.live[p.name]) return "em andamento";
    if (!p.lastRun) return "nunca executado";
    return { ok: "última execução sem falhas", partial: "última execução com falhas",
      error: "última execução falhou", cancelled: "última execução cancelada" }[p.lastRun.class] || "";
  }

  function renderSidebar() {
    const list = $("account-list");
    clear(list);
    $("accounts-title").hidden = state.profiles.length === 0;
    for (const p of state.profiles) {
      const btn = h("button", {
        type: "button", class: "account-item",
        "aria-current": p.name === state.selected ? "true" : "false",
        "data-name": p.name,
        onclick: () => select(p.name),
      },
      h("span", { class: `dot ${dotClass(p)}`, title: dotLabel(p) }),
      h("span", { class: "name", text: p.source.user || p.name }),
      h("span", { class: "sub", text: `${p.name} · ${dotLabel(p)}` }));
      list.append(h("li", {}, btn));
    }
  }

  // Arrow keys move between accounts.
  $("account-list").addEventListener("keydown", (e) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const items = [...document.querySelectorAll(".account-item")];
    const i = items.indexOf(document.activeElement);
    if (i < 0) return;
    e.preventDefault();
    const next = items[Math.max(0, Math.min(items.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
    next.focus();
    select(next.dataset.name);
  });

  // ---------- main panel ----------
  async function refresh(keepSelection = true) {
    try {
      state.profiles = await invoke("list_profiles");
    } catch (e) {
      toast(errText(e));
      return;
    }
    if (!keepSelection || !current()) state.selected = state.profiles.length ? state.profiles[0].name : null;
    renderSidebar();
    renderMain();
  }

  async function loadStatus(name) {
    const p = state.profiles.find((x) => x.name === name);
    if (!p || p.running || state.live[name] || !p.archiveExists) {
      delete state.status[name];
      if (name === state.selected) renderFolders();
      return;
    }
    try {
      state.status[name] = await invoke("get_status", { name });
    } catch (e) {
      state.status[name] = { error: errText(e) };
    }
    if (name === state.selected) renderFolders();
  }

  function select(name) {
    if (state.selected === name) return;
    state.selected = name;
    renderSidebar();
    renderMain();
    loadStatus(name);
  }

  function renderMain() {
    const p = current();
    $("empty-state").hidden = state.profiles.length > 0;
    $("account-view").hidden = !p;
    if (!p) return;

    $("acct-avatar").textContent = (p.source.user || p.name).slice(0, 1).toUpperCase();
    $("acct-title").textContent = p.source.user || p.name;
    $("acct-sub").textContent = p.error ? p.error : `${p.source.host}:${p.source.port} · conta "${p.name}"`;

    const busy = !!(p.running || state.live[p.name]);
    const pill = $("acct-pill");
    const cls = busy ? "running" : dotClass(p);
    pill.className = `pill ${cls}`;
    pill.textContent = busy ? `${OPS[(state.live[p.name] && state.live[p.name].progress && state.live[p.name].progress.op) || p.running] || "Operação"} em andamento`
      : !p.lastRun ? "Ainda não arquivado"
      : { ok: "Tudo certo", partial: "Concluído com falhas", error: "Último erro", cancelled: "Cancelado" }[p.lastRun.class] || "";

    for (const id of ["btn-archive", "btn-export", "btn-migrate", "btn-verify", "btn-reindex", "btn-edit", "btn-remove"]) {
      $(id).disabled = busy || !!p.error;
    }
    $("btn-verify").disabled = $("btn-verify").disabled || !p.archiveExists;
    $("btn-reindex").disabled = $("btn-reindex").disabled || !p.archiveExists;
    $("btn-open").disabled = !p.archiveExists;
    $("btn-open").title = p.archiveExists ? p.archiveDir : "A pasta é criada no primeiro arquivamento";

    $("st-messages").textContent = p.archiveExists ? num(p.messages) : "0";
    $("st-folders").textContent = p.archiveExists ? num(p.folderCount) : "0";
    $("st-last").textContent = when(p.lastArchiveAt);
    $("st-target").textContent = targetText(p);

    renderConfig(p);
    renderProgress();
    renderResult();
    renderFolders();
  }

  function targetText(p) {
    if (p.targetKind === "eml") return `Pasta .eml: ${p.targetDir}`;
    if (p.targetKind === "imap" && p.targetEndpoint) return `IMAP: ${p.targetEndpoint.user} em ${p.targetEndpoint.host}`;
    return "Não definido";
  }

  function renderConfig(p) {
    const dl = $("config-list");
    clear(dl);
    const tls = (t) => (t === "starttls" ? "STARTTLS" : "SSL/TLS");
    const rows = [
      ["Servidor", `${p.source.host}:${p.source.port} (${tls(p.source.tls)})`],
      ["Usuário", p.source.user],
      ["Pastas", p.folders && p.folders.length ? p.folders.join(", ") : "Todas"],
      ["Desde", p.since ? new Date(p.since + "T00:00:00").toLocaleDateString("pt-BR") : "Sem filtro de data"],
      ["Pasta do backup", p.archiveDir],
      ["Exportação", targetText(p)],
    ];
    for (const [k, v] of rows) dl.append(h("dt", { text: k }), h("dd", { text: v }));
  }

  function renderFolders() {
    const p = current();
    if (!p) return;
    const body = $("folder-rows");
    clear(body);
    const table = body.closest(".table-wrap");
    const empty = $("folders-empty");
    const note = $("folders-note");
    note.textContent = "";
    const st = state.status[p.name];
    let msg = "";
    if (!p.archiveExists) msg = "Ainda não há backup. Clique em \"Arquivar agora\" para começar.";
    else if (p.running || state.live[p.name]) msg = "A contagem por pasta aparece quando a operação terminar.";
    else if (!st) msg = "Carregando…";
    else if (st.error) msg = st.error;
    else if (!st.folders.length) msg = "O backup ainda está vazio.";
    if (msg) {
      table.hidden = true;
      empty.hidden = false;
      empty.textContent = msg;
      return;
    }
    table.hidden = false;
    empty.hidden = true;
    for (const f of st.folders) {
      body.append(h("tr", {}, h("td", { text: f.folder }), h("td", { class: "num", text: num(f.messages) })));
    }
    if (st.total !== st.unique) note.textContent = `${num(st.unique)} mensagens distintas (algumas estão em mais de uma pasta)`;
  }

  // ---------- progress ----------
  function phaseTitle(pr) {
    const op = OPS[pr.op] || "Operação";
    if (pr.cancelling) return `${op}: cancelando…`;
    if (pr.phase === "connect") return `${op}: conectando`;
    if (pr.op === "migrate") return pr.phase === "export" ? "Migração: exportando (2/2)" : "Migração: arquivando (1/2)";
    return `${op} em andamento`;
  }

  function renderProgress() {
    const p = current();
    const live = p && state.live[p.name];
    $("progress-card").hidden = !live;
    if (!live) return;
    const pr = live.progress;
    $("progress-title").textContent = phaseTitle(pr);
    $("progress-sub").textContent = pr.cancelling
      ? "O cancelamento acontece no próximo contato com o servidor. O que já foi gravado fica no backup."
      : pr.message || (pr.folder ? `Pasta: ${pr.folder}` : "Preparando…");
    const bar = $("progress-bar");
    const determinate = pr.phase !== "connect" && pr.folder && pr.total > 0;
    const pct = determinate ? Math.min(100, Math.round((pr.done / pr.total) * 100)) : 0;
    bar.classList.toggle("indeterminate", !determinate);
    $("progress-fill").style.width = determinate ? `${pct}%` : "";
    if (determinate) {
      bar.setAttribute("aria-valuenow", String(pct));
      bar.setAttribute("aria-valuetext", `${pct}% da pasta ${pr.folder}`);
    } else {
      bar.removeAttribute("aria-valuenow");
      bar.setAttribute("aria-valuetext", "Aguardando o servidor");
    }
    $("pc-folder").textContent = pr.folder || "—";
    $("pc-folder").title = pr.folder || "";
    $("pc-folder-count").textContent = pr.folder ? `${num(pr.done)} de ${num(pr.total)}` : "—";
    $("pc-processed").textContent = num(pr.processed);
    $("pc-folders").textContent = num(pr.foldersDone);
    $("pc-failed").textContent = num(pr.failed);
    const btn = $("btn-cancel");
    btn.disabled = !!pr.cancelling;
    btn.textContent = pr.cancelling ? "Cancelando…" : "Cancelar";

    const wrap = $("live-log-wrap");
    wrap.hidden = live.failures.length === 0;
    $("live-log-count").textContent = num(pr.failed);
    const ul = $("live-log");
    if (ul.dataset.profile !== p.name || Number(ul.dataset.count || 0) > live.failures.length) {
      clear(ul);
      ul.dataset.count = "0";
      ul.dataset.profile = p.name;
    }
    for (const f of live.failures.slice(Number(ul.dataset.count))) ul.append(failureItem(f));
    ul.dataset.count = String(live.failures.length);
  }

  function failureItem(f) {
    const where = f.uid === null || f.uid === undefined
      ? `Pasta ${f.folder}` : `${f.folder} · mensagem UID ${f.uid}`;
    return h("li", {}, h("span", { class: "where", text: where }), " — ", f.reason);
  }

  // ---------- result banner ----------
  function showResult(name, r) {
    state.results[name] = r;
    if (name === state.selected) renderResult();
  }

  function renderResult() {
    const p = current();
    const r = p && state.results[p.name];
    const el = $("result");
    el.hidden = !r;
    if (!r) return;
    el.className = `banner ${r.kind}`;
    el.querySelector(".banner-icon").textContent = { ok: "✓", partial: "!", error: "×", cancelled: "–", info: "i" }[r.kind] || "i";
    $("result-title").textContent = r.title;
    $("result-text").textContent = r.text || "";
    const det = $("result-details");
    const ul = $("result-failures");
    clear(ul);
    const items = r.failures || [];
    det.hidden = items.length === 0;
    if (items.length) {
      $("result-details-summary").textContent = r.total > items.length
        ? `Ver falhas (mostrando ${num(items.length)} de ${num(r.total)})` : `Ver falhas (${num(items.length)})`;
      for (const f of items) ul.append(typeof f === "string" ? h("li", { text: f }) : failureItem(f));
    }
  }
  $("result-close").addEventListener("click", () => {
    if (state.selected) delete state.results[state.selected];
    renderResult();
  });

  function finishedToResult(f) {
    const op = OPS[f.op] || "Operação";
    const parts = [];
    for (const ph of f.phases) {
      const label = ph.phase === "export" ? "Exportação" : "Arquivamento";
      if (ph.error && ph.class !== "cancelled") continue;
      if (ph.phase === "export") parts.push(`${label}: ${num(ph.added)} enviadas, ${num(ph.duplicates)} já estavam no destino.`);
      else parts.push(`${label}: ${num(ph.added)} novas mensagens, ${num(ph.duplicates)} já estavam no backup.`);
    }
    const summary = parts.join("\n");
    if (f.class === "ok") {
      return { kind: "ok", title: `${op} concluído com sucesso`, text: summary, failures: [] };
    }
    if (f.class === "partial") {
      return { kind: "partial", title: `${op} concluído com falhas (${num(f.failed)})`,
        text: `${summary}\nAs mensagens que falharam serão tentadas de novo na próxima execução.`.trim(),
        failures: f.failures, total: f.failuresTotal };
    }
    if (f.class === "cancelled") {
      return { kind: "cancelled", title: `${op} cancelado`, text: [f.error, summary].filter(Boolean).join("\n"),
        failures: f.failures, total: f.failuresTotal };
    }
    return { kind: "error", title: `${op} interrompido por um erro`,
      text: [f.error, summary].filter(Boolean).join("\n"), failures: f.failures, total: f.failuresTotal };
  }

  // ---------- events from Rust ----------
  listen("progress", (ev) => {
    const pr = ev.payload;
    const isNew = !state.live[pr.profile];
    const live = state.live[pr.profile] || (state.live[pr.profile] = { progress: pr, failures: [] });
    live.progress = pr;
    if (isNew) { renderSidebar(); if (pr.profile === state.selected) renderMain(); }
    if (pr.newFailures && pr.newFailures.length) {
      live.failures.push(...pr.newFailures);
      if (live.failures.length > 300) live.failures.splice(0, live.failures.length - 300);
    }
    if (pr.profile === state.selected) renderProgress();
  });

  listen("finished", async (ev) => {
    const f = ev.payload;
    delete state.live[f.profile];
    showResult(f.profile, finishedToResult(f));
    await refresh();
    loadStatus(f.profile);
    const who = state.profiles.find((p) => p.name === f.profile);
    if (f.profile !== state.selected) toast(`${OPS[f.op]} de ${who ? who.source.user : f.profile} terminou.`);
  });

  // ---------- actions ----------
  async function startOp(op) {
    const p = current();
    if (!p) return;
    const cmd = { archive: "start_archive", export: "start_export", migrate: "start_migrate" }[op];
    try {
      delete state.results[p.name];
      state.live[p.name] = { progress: { profile: p.name, op, phase: "connect", folder: null, done: 0, total: 0, processed: 0, failed: 0, foldersDone: 0, newFailures: [], message: "Iniciando…", cancelling: false }, failures: [] };
      renderSidebar();
      renderMain();
      await invoke(cmd, { name: p.name });
      $("btn-cancel").focus();
    } catch (e) {
      delete state.live[p.name];
      showResult(p.name, { kind: "error", title: `Não foi possível iniciar: ${OPS[op].toLowerCase()}`, text: errText(e) });
      renderSidebar();
      renderMain();
    }
  }

  $("btn-archive").addEventListener("click", () => startOp("archive"));
  $("btn-export").addEventListener("click", () => openExport("export"));
  $("btn-migrate").addEventListener("click", async () => {
    const p = current();
    if (!p) return;
    if (p.targetKind === "none") return openExport("migrate");
    const ok = await confirmDialog("Migrar esta conta?",
      `Primeiro arquiva as mensagens novas de ${p.source.user} e depois copia o backup para:\n${targetText(p)}\n\nO que já foi enviado antes não é enviado de novo.`,
      "Migrar agora");
    if (ok) startOp("migrate");
  });
  $("btn-cancel").addEventListener("click", async () => {
    const p = current();
    if (!p) return;
    try {
      await invoke("cancel", { name: p.name });
      const live = state.live[p.name];
      if (live) { live.progress.cancelling = true; renderProgress(); }
    } catch (e) {
      toast(errText(e));
    }
  });
  $("btn-verify").addEventListener("click", async () => {
    const p = current();
    const btn = $("btn-verify");
    setBusy(btn, true, "Verificando…");
    try {
      const r = await invoke("run_verify", { name: p.name });
      const bad = r.missing.length + r.corrupt.length;
      showResult(p.name, bad === 0
        ? { kind: "ok", title: "Verificação concluída: nenhum problema", text: `${num(r.checked)} arquivos .eml conferidos com o índice.` }
        : { kind: "error", title: `Verificação encontrou ${num(bad)} problema(s)`,
          text: `${num(r.checked)} conferidos · ${num(r.missing.length)} ausentes · ${num(r.corrupt.length)} corrompidos.\nArquive de novo para baixar outra vez as mensagens que ainda estão no servidor.`,
          failures: [...r.missing.map((s) => `Ausente: ${s}`), ...r.corrupt.map((s) => `Corrompido: ${s}`)] });
    } catch (e) {
      showResult(p.name, { kind: "error", title: "Não foi possível verificar", text: errText(e) });
    } finally {
      setBusy(btn, false, "Verificando…");
    }
  });
  $("btn-reindex").addEventListener("click", async () => {
    const p = current();
    const ok = await confirmDialog("Reconstruir o índice?",
      "Recria o índice (index.db) a partir dos arquivos .eml. Use se o índice sumiu ou foi danificado.\n\nDepois disso:\n• arquive de novo para recuperar UIDs, flags e pastas;\n• não exporte de novo para um servidor IMAP que já recebeu as mensagens (elas seriam reenviadas e duplicadas).",
      "Reconstruir");
    if (!ok) return;
    const btn = $("btn-reindex");
    setBusy(btn, true, "Reconstruindo…");
    try {
      const n = await invoke("run_reindex", { name: p.name });
      showResult(p.name, { kind: "ok", title: "Índice reconstruído", text: `${num(n)} mensagens indexadas. Arquive de novo para restaurar UIDs e flags.` });
      await refresh();
      loadStatus(p.name);
    } catch (e) {
      showResult(p.name, { kind: "error", title: "Não foi possível reconstruir o índice", text: errText(e) });
    } finally {
      setBusy(btn, false, "Reconstruindo…");
    }
  });
  $("btn-open").addEventListener("click", async () => {
    const p = current();
    try { await invoke("open_folder", { path: p.archiveDir }); } catch (e) { toast(errText(e)); }
  });
  $("btn-remove").addEventListener("click", async () => {
    const p = current();
    const ok = await confirmDialog("Remover esta conta?",
      `A conta "${p.name}" (${p.source.user}) será removida do MailBackup e as senhas serão apagadas do Gerenciador de Credenciais do Windows.\n\nA pasta do backup NÃO será apagada:\n${p.archiveDir}`,
      "Remover conta", true);
    if (!ok) return;
    try {
      await invoke("delete_profile", { name: p.name });
      delete state.results[p.name];
      delete state.status[p.name];
      toast("Conta removida. A pasta do backup foi mantida.");
      await refresh(false);
    } catch (e) {
      showResult(p.name, { kind: "error", title: "Não foi possível remover a conta", text: errText(e) });
    }
  });
  $("btn-edit").addEventListener("click", () => openWizard(current()));
  $("add-account").addEventListener("click", () => openWizard(null));
  $("empty-add").addEventListener("click", () => openWizard(null));

  // ---------- confirmation dialog ----------
  function confirmDialog(title, text, yes, danger = false) {
    const d = $("confirm");
    $("cf-title").textContent = title;
    $("cf-text").textContent = text;
    const y = $("cf-yes");
    y.textContent = yes;
    y.className = `btn ${danger ? "btn-danger" : "btn-primary"}`;
    return new Promise((resolve) => {
      const done = (v) => { cleanup(); d.close(); resolve(v); };
      const onYes = () => done(true);
      const onNo = () => done(false);
      const onCancel = (e) => { e.preventDefault(); done(false); };
      function cleanup() {
        y.removeEventListener("click", onYes);
        $("cf-no").removeEventListener("click", onNo);
        d.removeEventListener("cancel", onCancel);
      }
      y.addEventListener("click", onYes);
      $("cf-no").addEventListener("click", onNo);
      d.addEventListener("cancel", onCancel);
      d.showModal();
      $("cf-no").focus();
    });
  }

  // ---------- password show/hide ----------
  function pwToggle(btnId, inputId) {
    $(btnId).addEventListener("click", () => {
      const input = $(inputId);
      const show = input.type === "password";
      input.type = show ? "text" : "password";
      $(btnId).textContent = show ? "Ocultar" : "Mostrar";
      $(btnId).setAttribute("aria-pressed", String(show));
      input.focus();
    });
  }
  pwToggle("wz-pw-toggle", "wz-password");
  pwToggle("ex-pw-toggle", "ex-password");

  // ---------- add / edit wizard ----------
  const wz = {
    step: 1,
    editing: null,      // profile being edited, or null
    folders: null,      // folder list from the last successful test
    testedKey: "",      // connection fields of that test
    detectedEmail: "",
    nameTouched: false,
    dirTouched: false,
    busy: false,
  };

  function wzEndpoint() {
    return {
      host: $("wz-host").value.trim(),
      port: Number($("wz-port").value) || 0,
      tls: $("wz-tls").value,
      user: $("wz-user").value.trim(),
    };
  }
  const epKey = (ep, pw) => JSON.stringify([ep.host, ep.port, ep.tls, ep.user, pw ? pw.length : 0, pw]);

  function wzError(msg) {
    const el = $("wz-error");
    el.hidden = !msg;
    el.textContent = msg || "";
  }

  function updateServerSummary() {
    const ep = wzEndpoint();
    $("wz-server-summary").textContent = ep.host ? `— ${ep.host}:${ep.port || "?"} · ${ep.tls === "starttls" ? "STARTTLS" : "SSL/TLS"}` : "";
  }

  function openWizard(editing) {
    wz.step = 1;
    wz.editing = editing;
    wz.folders = null;
    wz.testedKey = "";
    wz.detectedEmail = "";
    wz.nameTouched = !!editing;
    wz.dirTouched = !!editing;
    $("wz-title").textContent = editing ? `Editar conta "${editing.name}"` : "Adicionar conta";
    $("wz-email").value = editing ? editing.source.user : "";
    $("wz-email").disabled = false;
    $("wz-password").value = "";
    $("wz-password").type = "password";
    $("wz-pw-toggle").textContent = "Mostrar";
    $("wz-pw-help").textContent = editing
      ? "Deixe em branco para manter a senha guardada no Gerenciador de Credenciais do Windows."
      : "A senha é guardada no Gerenciador de Credenciais do Windows, nunca em arquivo.";
    $("wz-host").value = editing ? editing.source.host : "";
    $("wz-port").value = editing ? editing.source.port : 993;
    $("wz-tls").value = editing ? editing.source.tls : "implicit";
    $("wz-user").value = editing ? editing.source.user : "";
    $("wz-advanced").open = !!editing;
    $("wz-hint").hidden = true;
    setTest("wz-test-result", "", "");
    $("wz-since-on").checked = !!(editing && editing.since);
    $("wz-since").value = editing && editing.since ? editing.since : "";
    $("wz-since").disabled = !$("wz-since-on").checked;
    $("wz-name").value = editing ? editing.name : "";
    $("wz-name").disabled = !!editing;
    $("wz-dir").value = editing ? editing.archiveDir : "";
    $("wz-run-now").checked = !editing;
    $("wz-run-wrap").hidden = !!editing;
    clear($("wz-tree"));
    wzError("");
    updateServerSummary();
    showStep(1);
    $("wizard").showModal();
    $("wz-email").focus();
  }

  function showStep(n) {
    wz.step = n;
    for (const s of document.querySelectorAll(".wz-step")) s.hidden = Number(s.dataset.step) !== n;
    for (const li of document.querySelectorAll("#wz-steps li")) {
      const k = Number(li.dataset.step);
      li.className = k === n ? "current" : k < n ? "done" : "";
      if (k === n) li.setAttribute("aria-current", "step"); else li.removeAttribute("aria-current");
    }
    $("wz-back").hidden = n === 1;
    $("wz-next").textContent = n === 4 ? (wz.editing ? "Salvar" : "Concluir") : "Avançar";
    wzError("");
    if (n === 4) renderSummary();
    const first = document.querySelector(`.wz-step[data-step="${n}"] input:not([disabled]), .wz-step[data-step="${n}"] button`);
    if (first && n !== 1) first.focus();
  }

  let detectTimer = 0;
  $("wz-email").addEventListener("input", () => {
    clearTimeout(detectTimer);
    detectTimer = setTimeout(detect, 350);
  });
  $("wz-email").addEventListener("change", detect);

  async function detect() {
    const email = $("wz-email").value.trim();
    if (!email || email === wz.detectedEmail || !email.includes("@")) return;
    try {
      const d = await invoke("detect_server", { email });
      if ($("wz-email").value.trim() !== email) return;
      wz.detectedEmail = email;
      if (!wz.editing) {
        $("wz-host").value = d.endpoint.host;
        $("wz-port").value = d.endpoint.port;
        $("wz-tls").value = d.endpoint.tls;
        $("wz-user").value = d.endpoint.user;
        if (!wz.nameTouched) $("wz-name").value = d.suggestedName;
        if (!wz.dirTouched) $("wz-dir").value = d.suggestedDir;
      }
      $("wz-hint").hidden = !d.hint;
      $("wz-hint").textContent = d.hint || "";
      updateServerSummary();
    } catch (_) {
      // not an address yet: keep what the user typed
    }
  }
  for (const id of ["wz-host", "wz-port", "wz-tls", "wz-user"]) $(id).addEventListener("input", updateServerSummary);
  $("wz-tls").addEventListener("change", () => {
    const port = $("wz-port");
    if ($("wz-tls").value === "starttls" && port.value === "993") port.value = "143";
    if ($("wz-tls").value === "implicit" && port.value === "143") port.value = "993";
    updateServerSummary();
  });
  $("wz-name").addEventListener("input", () => { wz.nameTouched = true; });
  $("wz-dir").addEventListener("input", () => { wz.dirTouched = true; });

  function setTest(id, kind, text) {
    const el = $(id);
    el.className = `test-result ${kind}`;
    el.textContent = text;
    if (text) el.scrollIntoView({ block: "nearest" });
  }

  async function wzTest() {
    const ep = wzEndpoint();
    const pw = $("wz-password").value;
    if (!ep.host || !ep.user) {
      $("wz-advanced").open = true;
      setTest("wz-test-result", "error", "Preencha o e-mail (ou o servidor e o usuário em \"Servidor\").");
      return false;
    }
    if (!pw && !wz.editing) {
      setTest("wz-test-result", "error", "Informe a senha.");
      $("wz-password").focus();
      return false;
    }
    const btn = $("wz-test");
    setBusy(btn, true, "Testando…");
    setTest("wz-test-result", "busy", `Conectando a ${ep.host}…`);
    try {
      const folders = pw
        ? await invoke("test_connection", { endpoint: ep, password: pw })
        : await invoke("test_saved_connection", { name: wz.editing.name, role: "source", endpoint: ep });
      wz.folders = folders;
      wz.testedKey = epKey(ep, pw);
      setTest("wz-test-result", "ok", `✓ Conexão bem-sucedida — ${folders.length} pasta(s) encontrada(s).`);
      buildTree(folders, wz.editing ? wz.editing.folders : null);
      return true;
    } catch (e) {
      wz.folders = null;
      let msg = errText(e);
      if (/gmail|office365|outlook|hotmail|live\.com/i.test(ep.host) && !/senha de app/.test(msg)) {
        msg += "\nDica: Gmail e Microsoft 365 exigem uma senha de app.";
      }
      setTest("wz-test-result", "error", msg);
      return false;
    } finally {
      setBusy(btn, false, "Testando…");
    }
  }
  $("wz-test").addEventListener("click", wzTest);

  // Folder tree: "/"-separated paths; parents that are not folders themselves only group.
  function buildTree(folders, selected) {
    const root = { children: new Map(), path: "", real: false };
    for (const f of folders) {
      let node = root;
      const parts = f.split("/");
      parts.forEach((part, i) => {
        const path = parts.slice(0, i + 1).join("/");
        if (!node.children.has(part)) node.children.set(part, { children: new Map(), path, real: false, name: part });
        node = node.children.get(part);
      });
      node.real = true;
    }
    const want = selected && selected.length ? new Set(selected.map((s) => s.toLowerCase())) : null;
    const tree = $("wz-tree");
    clear(tree);
    const sortKey = (n) => (n.path.toUpperCase() === "INBOX" ? "" : n.name.toLowerCase());
    function render(node, ul) {
      const kids = [...node.children.values()].sort((a, b) => sortKey(a).localeCompare(sortKey(b), "pt-BR"));
      for (const k of kids) {
        const cb = h("input", { type: "checkbox", "data-path": k.path, "data-real": k.real ? "1" : "0" });
        cb.checked = want ? want.has(k.path.toLowerCase()) : true;
        const label = h("label", { class: k.real ? "" : "group-label" }, cb,
          k.path.toUpperCase() === "INBOX" ? "Caixa de entrada (INBOX)" : k.name);
        const li = h("li", { role: "treeitem" }, label);
        if (k.children.size) {
          const sub = h("ul", { role: "group" });
          render(k, sub);
          li.append(sub);
        }
        ul.append(li);
      }
    }
    render(root, tree);
    syncTree();
  }

  function treeBoxes() { return [...$("wz-tree").querySelectorAll("input[type=checkbox]")]; }
  function syncTree() {
    // parents reflect their children (checked / indeterminate)
    const boxes = treeBoxes().reverse();
    for (const cb of boxes) {
      const sub = cb.closest("li").querySelector(":scope > ul");
      if (!sub) continue;
      const kids = [...sub.querySelectorAll(":scope > li > label > input")];
      const on = kids.filter((k) => k.checked && !k.indeterminate).length;
      const some = kids.some((k) => k.checked || k.indeterminate);
      if (cb.dataset.real === "1") {
        cb.indeterminate = (on !== kids.length && some) || (!cb.checked && some);
      } else {
        cb.checked = on === kids.length;
        cb.indeterminate = !cb.checked && some;
      }
    }
    const real = treeBoxes().filter((b) => b.dataset.real === "1");
    const n = real.filter((b) => b.checked).length;
    $("wz-folder-count").textContent = `${n} de ${real.length} pasta(s) selecionada(s).`;
  }
  $("wz-tree").addEventListener("change", (e) => {
    const cb = e.target;
    const sub = cb.closest("li").querySelector(":scope > ul");
    if (sub) for (const k of sub.querySelectorAll("input[type=checkbox]")) { k.checked = cb.checked; k.indeterminate = false; }
    cb.indeterminate = false;
    syncTree();
  });
  $("wz-all").addEventListener("click", () => { for (const b of treeBoxes()) { b.checked = true; b.indeterminate = false; } syncTree(); });
  $("wz-none").addEventListener("click", () => { for (const b of treeBoxes()) { b.checked = false; b.indeterminate = false; } syncTree(); });
  $("wz-since-on").addEventListener("change", () => {
    $("wz-since").disabled = !$("wz-since-on").checked;
    if ($("wz-since-on").checked) $("wz-since").focus();
  });

  function selectedFolders() {
    const real = treeBoxes().filter((b) => b.dataset.real === "1");
    const chosen = real.filter((b) => b.checked).map((b) => b.dataset.path);
    return { all: chosen.length === real.length, chosen };
  }

  async function pick(inputId, title) {
    try {
      const dir = await invoke("pick_folder", { title, start: $(inputId).value || null });
      if (dir) { $(inputId).value = dir; $(inputId).dispatchEvent(new Event("input")); }
    } catch (e) {
      toast(errText(e));
    }
  }
  $("wz-dir-pick").addEventListener("click", () => pick("wz-dir", "Pasta do backup"));

  function renderSummary() {
    const dl = $("wz-summary");
    clear(dl);
    const ep = wzEndpoint();
    const { all, chosen } = selectedFolders();
    const rows = [
      ["Conta", $("wz-name").value.trim()],
      ["E-mail / usuário", ep.user],
      ["Servidor", `${ep.host}:${ep.port} (${ep.tls === "starttls" ? "STARTTLS" : "SSL/TLS"})`],
      ["Senha", $("wz-password").value ? "Será guardada no Gerenciador de Credenciais do Windows" : "Mantida (já guardada)"],
      ["Pastas", all ? `Todas (${chosen.length})` : chosen.join(", ")],
      ["Desde", $("wz-since-on").checked && $("wz-since").value ? new Date($("wz-since").value + "T00:00:00").toLocaleDateString("pt-BR") : "Sem filtro de data"],
      ["Pasta do backup", $("wz-dir").value.trim()],
    ];
    for (const [k, v] of rows) dl.append(h("dt", { text: k }), h("dd", { text: v }));
  }

  async function wzNext() {
    if (wz.busy) return;
    wzError("");
    if (wz.step === 1) {
      const ep = wzEndpoint();
      const pw = $("wz-password").value;
      if (!wz.folders || wz.testedKey !== epKey(ep, pw)) {
        wz.busy = true;
        const ok = await wzTest();
        wz.busy = false;
        if (!ok) return;
      }
      return showStep(2);
    }
    if (wz.step === 2) {
      const { chosen } = selectedFolders();
      if (!chosen.length) return wzError("Marque pelo menos uma pasta.");
      if ($("wz-since-on").checked && !$("wz-since").value) return wzError("Escolha a data ou desmarque \"Arquivar apenas e-mails desde\".");
      if (!$("wz-name").value) $("wz-name").value = "conta";
      return showStep(3);
    }
    if (wz.step === 3) {
      const name = $("wz-name").value.trim();
      if (!/^[A-Za-z0-9._-]{1,64}$/.test(name) || name.startsWith(".") || name.endsWith(".")) {
        $("wz-name").setAttribute("aria-invalid", "true");
        return wzError("Nome inválido: use letras sem acento, números, '.', '_' ou '-' (sem espaços).");
      }
      $("wz-name").removeAttribute("aria-invalid");
      const clash = !wz.editing && state.profiles.find((p) => p.name.toLowerCase() === name.toLowerCase());
      if (clash) return wzError(`Já existe uma conta chamada "${clash.name}". Escolha outro nome.`);
      if (!$("wz-dir").value.trim()) return wzError("Escolha a pasta do backup.");
      return showStep(4);
    }
    // step 4: save
    const { all, chosen } = selectedFolders();
    const input = {
      name: $("wz-name").value.trim(),
      source: wzEndpoint(),
      password: $("wz-password").value || null,
      folders: all ? null : chosen,
      since: $("wz-since-on").checked ? $("wz-since").value : null,
      archiveDir: $("wz-dir").value.trim(),
      isNew: !wz.editing,
    };
    const btn = $("wz-next");
    wz.busy = true;
    setBusy(btn, true, "Salvando…");
    try {
      await invoke("save_profile", { input });
      $("wz-password").value = "";
      $("wizard").close();
      state.selected = input.name;
      await refresh();
      loadStatus(input.name);
      if (!wz.editing && $("wz-run-now").checked) startOp("archive");
      else toast(wz.editing ? "Conta atualizada." : "Conta adicionada.");
    } catch (e) {
      wzError(errText(e));
    } finally {
      wz.busy = false;
      setBusy(btn, false, "Salvando…");
    }
  }
  $("wz-next").addEventListener("click", wzNext);
  $("wz-back").addEventListener("click", () => { if (wz.step > 1) showStep(wz.step - 1); });
  $("wz-cancel").addEventListener("click", () => { $("wz-password").value = ""; $("wizard").close(); });
  $("wizard").addEventListener("close", () => { $("wz-password").value = ""; wz.testedKey = ""; });
  $("wz-form").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.tagName === "INPUT" && e.target.type !== "checkbox") {
      e.preventDefault();
      wzNext();
    }
  });

  // ---------- export destination dialog ----------
  const ex = { mode: "export", profile: null };
  const exKind = () => document.querySelector("input[name=ex-kind]:checked").value;
  function exEndpoint() {
    return { host: $("ex-host").value.trim(), port: Number($("ex-port").value) || 0, tls: $("ex-tls").value, user: $("ex-user").value.trim() };
  }
  function exError(msg) { $("ex-error").hidden = !msg; $("ex-error").textContent = msg || ""; }
  function syncExKind() {
    const k = exKind();
    $("ex-eml").hidden = k !== "eml";
    $("ex-imap").hidden = k !== "imap";
  }
  for (const r of document.querySelectorAll("input[name=ex-kind]")) r.addEventListener("change", syncExKind);

  function openExport(mode) {
    const p = current();
    if (!p) return;
    ex.mode = mode;
    ex.profile = p;
    $("ex-title").textContent = mode === "migrate" ? "Migrar: escolha o destino" : "Exportar";
    $("ex-run").textContent = mode === "migrate" ? "Salvar e migrar agora" : "Salvar e exportar agora";
    const kind = p.targetKind === "imap" ? "imap" : "eml";
    document.querySelector(`input[name=ex-kind][value=${kind}]`).checked = true;
    $("ex-dir").value = p.targetDir || (p.archiveDir ? `${p.archiveDir}-eml` : "");
    const t = p.targetEndpoint;
    $("ex-email").value = t ? t.user : "";
    $("ex-host").value = t ? t.host : "";
    $("ex-port").value = t ? t.port : 993;
    $("ex-tls").value = t ? t.tls : "implicit";
    $("ex-user").value = t ? t.user : "";
    $("ex-password").value = "";
    $("ex-password").type = "password";
    $("ex-pw-toggle").textContent = "Mostrar";
    $("ex-pw-help").textContent = t ? "Deixe em branco para manter a senha já guardada." : "Guardada no Gerenciador de Credenciais do Windows.";
    $("ex-hint").hidden = true;
    setTest("ex-test-result", "", "");
    exError("");
    syncExKind();
    $("export-dialog").showModal();
    document.querySelector("input[name=ex-kind]:checked").focus();
  }

  let exDetectTimer = 0;
  $("ex-email").addEventListener("input", () => {
    clearTimeout(exDetectTimer);
    exDetectTimer = setTimeout(async () => {
      const email = $("ex-email").value.trim();
      if (!email.includes("@")) return;
      try {
        const d = await invoke("detect_server", { email });
        $("ex-host").value = d.endpoint.host;
        $("ex-port").value = d.endpoint.port;
        $("ex-tls").value = d.endpoint.tls;
        $("ex-user").value = d.endpoint.user;
        $("ex-hint").hidden = !d.hint;
        $("ex-hint").textContent = d.hint || "";
      } catch (_) { /* keep fields */ }
    }, 350);
  });
  $("ex-tls").addEventListener("change", () => {
    const port = $("ex-port");
    if ($("ex-tls").value === "starttls" && port.value === "993") port.value = "143";
    if ($("ex-tls").value === "implicit" && port.value === "143") port.value = "993";
  });
  $("ex-dir-pick").addEventListener("click", () => pick("ex-dir", "Pasta de destino da exportação"));
  $("ex-test").addEventListener("click", async () => {
    const ep = exEndpoint();
    const pw = $("ex-password").value;
    const p = ex.profile;
    if (!ep.host || !ep.user) return setTest("ex-test-result", "error", "Preencha o servidor e o usuário.");
    if (!pw && p.targetKind !== "imap") return setTest("ex-test-result", "error", "Informe a senha.");
    const btn = $("ex-test");
    setBusy(btn, true, "Testando…");
    setTest("ex-test-result", "busy", `Conectando a ${ep.host}…`);
    try {
      const folders = pw
        ? await invoke("test_connection", { endpoint: ep, password: pw })
        : await invoke("test_saved_connection", { name: p.name, role: "target", endpoint: ep });
      setTest("ex-test-result", "ok", `✓ Conexão bem-sucedida — ${folders.length} pasta(s) no destino.`);
    } catch (e) {
      setTest("ex-test-result", "error", errText(e));
    } finally {
      setBusy(btn, false, "Testando…");
    }
  });

  async function exSave(run) {
    const p = ex.profile;
    const kind = exKind();
    const target = kind === "eml"
      ? { kind, dir: $("ex-dir").value.trim(), endpoint: null, password: null }
      : { kind, dir: null, endpoint: exEndpoint(), password: $("ex-password").value || null };
    const btns = [$("ex-save"), $("ex-run")];
    btns.forEach((b) => { b.disabled = true; });
    exError("");
    try {
      await invoke("set_target", { name: p.name, target });
      $("ex-password").value = "";
      $("export-dialog").close();
      await refresh();
      if (run) startOp(ex.mode === "migrate" ? "migrate" : "export");
      else toast("Destino de exportação salvo.");
    } catch (e) {
      exError(errText(e));
    } finally {
      btns.forEach((b) => { b.disabled = false; });
    }
  }
  $("ex-save").addEventListener("click", () => exSave(false));
  $("ex-run").addEventListener("click", () => exSave(true));
  $("ex-cancel").addEventListener("click", () => { $("ex-password").value = ""; $("export-dialog").close(); });
  $("export-dialog").addEventListener("close", () => { $("ex-password").value = ""; });

  // Keep "last archive" relative times fresh.
  setInterval(() => { const p = current(); if (p) $("st-last").textContent = when(p.lastArchiveAt); }, 60000);

  // ---------- start ----------
  refresh(false).then(() => {
    for (const p of state.profiles) {
      if (p.running) state.live[p.name] = { progress: { profile: p.name, op: p.running, phase: "connect", folder: null, done: 0, total: 0, processed: 0, failed: 0, foldersDone: 0, newFailures: [], message: "Em andamento…", cancelling: false }, failures: [] };
    }
    if (state.selected) loadStatus(state.selected);
  });
})();
