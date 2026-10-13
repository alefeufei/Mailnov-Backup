// DEV ONLY: a fake `window.__TAURI__` so the UI in ../ui can be opened in a normal browser
// (no Rust, no mail server, no credential store) to check layout and states.
// Not part of the app: this folder is outside `frontendDist`.
// Usage: dev/preview.html#<scenario>; scenarios are listed in SCENARIOS below.
"use strict";
(() => {
  const now = Math.floor(Date.now() / 1000);
  const ep = (host, user, port = 993, tls = "implicit") => ({ host, port, tls, user });
  const profiles = [
    {
      name: "joao-gmail", source: ep("imap.gmail.com", "joao.silva@gmail.com"),
      archiveDir: "C:\\Users\\Joao\\Documents\\MailBackup\\joao-gmail", folders: null, since: null,
      targetKind: "eml", targetDir: "D:\\Exportado\\joao", targetEndpoint: null,
      archiveExists: true, messages: 18342, folderCount: 9, lastArchiveAt: now - 3 * 3600,
      lastRun: { op: "archive", class: "ok", finishedAt: now - 3 * 3600, added: 112, failed: 0 },
      running: null, error: null,
    },
    {
      name: "trabalho", source: ep("mail.empresa.com.br", "joao@empresa.com.br", 143, "starttls"),
      archiveDir: "D:\\Backups\\trabalho", folders: ["INBOX", "Projetos/2024"], since: "2023-01-01",
      targetKind: "imap", targetDir: null, targetEndpoint: ep("outlook.office365.com", "joao@novaempresa.com"),
      archiveExists: true, messages: 4210, folderCount: 2, lastArchiveAt: now - 86400 * 2,
      lastRun: { op: "archive", class: "partial", finishedAt: now - 86400 * 2, added: 40, failed: 3 },
      running: null, error: null,
    },
    {
      name: "antigo-yahoo", source: ep("imap.mail.yahoo.com", "joao_antigo@yahoo.com"),
      archiveDir: "C:\\Users\\Joao\\Documents\\MailBackup\\antigo-yahoo", folders: null, since: null,
      targetKind: "none", targetDir: null, targetEndpoint: null,
      archiveExists: false, messages: null, folderCount: null, lastArchiveAt: null,
      lastRun: { op: "archive", class: "error", finishedAt: now - 600, added: 0, failed: 0 },
      running: null, error: null,
    },
  ];
  const remoteFolders = ["INBOX", "Enviados", "Rascunhos", "Lixeira", "Spam", "[Gmail]/Todos os e-mails",
    "[Gmail]/Com estrela", "Projetos", "Projetos/2023", "Projetos/2024", "Projetos/2024/Cliente <A&B>",
    "Pessoal", "Pessoal/Viagens"];
  const status = {
    folders: [
      ["INBOX", 8211], ["Enviados", 3120], ["Rascunhos", 12], ["Lixeira", 433], ["Spam", 87],
      ["[Gmail]/Todos os e-mails", 6110], ["Projetos/2024", 210], ["Pessoal", 98], ["Pessoal/Viagens", 41],
    ].map(([folder, messages]) => ({ folder, messages })),
  };
  status.total = status.folders.reduce((a, f) => a + f.messages, 0);
  status.unique = 18342;

  const scenario = (location.hash || "#accounts").slice(1);
  const listeners = {};
  const emit = (ev, payload) => (listeners[ev] || []).forEach((cb) => cb({ payload }));
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));

  const handlers = {
    list_profiles: () => (scenario === "empty" ? [] : profiles),
    get_status: () => status,
    detect_server: ({ email }) => {
      const domain = email.split("@")[1] || "";
      const host = domain === "gmail.com" ? "imap.gmail.com" : `imap.${domain}`;
      return {
        endpoint: ep(host, email),
        hint: domain === "gmail.com" ? "Gmail: ative o IMAP e use uma senha de app (Conta Google > Segurança > Senhas de app). A senha normal não funciona." : null,
        suggestedName: email.replace("@", "-").split(".")[0],
        suggestedDir: `C:\\Users\\Joao\\Documents\\MailBackup\\${email.replace("@", "-").split(".")[0]}`,
      };
    },
    test_connection: async () => {
      await wait(300);
      if (scenario === "wizard-error") {
        throw "O servidor recusou o login: usuário ou senha incorretos, ou o acesso IMAP está desativado nesta conta.\nDetalhe técnico: authentication error: login rejected by server: No Response: [AUTHENTICATIONFAILED] Invalid credentials (Failure)\nGmail: ative o IMAP e use uma senha de app (Conta Google > Segurança > Senhas de app). A senha normal não funciona.";
      }
      return remoteFolders;
    },
    test_saved_connection: () => remoteFolders,
    save_profile: () => null,
    set_target: () => null,
    delete_profile: () => null,
    start_archive: () => null,
    start_export: () => null,
    start_migrate: () => null,
    cancel: () => null,
    run_verify: () => ({ checked: 18342, missing: [], corrupt: [] }),
    run_reindex: () => 18342,
    open_folder: () => null,
    pick_folder: () => "D:\\Backups\\escolhida",
  };

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        const f = handlers[cmd];
        if (!f) throw `mock: unknown command ${cmd}`;
        return f(args || {});
      },
    },
    event: {
      listen: async (ev, cb) => { (listeners[ev] = listeners[ev] || []).push(cb); return () => {}; },
    },
  };

  // ---- scripted scenarios (for screenshots) ----
  const $ = (id) => document.getElementById(id);
  const click = (id) => $(id).click();
  const type = (id, v) => { const el = $(id); el.value = v; el.dispatchEvent(new Event("input")); el.dispatchEvent(new Event("change")); };
  const progress = (over) => Object.assign({
    profile: "joao-gmail", op: "archive", phase: "archive", folder: "INBOX", done: 640, total: 1500,
    processed: 2890, failed: 0, foldersDone: 3, newFailures: [], message: null, cancelling: false,
  }, over);

  const SCENARIOS = {
    empty: async () => {},
    accounts: async () => {},
    wizard1: async () => {
      click("add-account");
      type("wz-email", "joao.silva@gmail.com");
      type("wz-password", "senha-de-app");
      await wait(500);
      $("wz-advanced").open = true;
      click("wz-test");
    },
    "wizard-error": async () => {
      click("add-account");
      type("wz-email", "joao.silva@gmail.com");
      type("wz-password", "errada");
      await wait(500);
      click("wz-test");
    },
    wizard2: async () => {
      await SCENARIOS.wizard1();
      await wait(500);
      click("wz-next");
      await wait(100);
      for (const b of document.querySelectorAll('#wz-tree input[data-path="Spam"], #wz-tree input[data-path="Lixeira"]')) { b.checked = false; b.dispatchEvent(new Event("change", { bubbles: true })); }
      $("wz-since-on").click();
      type("wz-since", "2020-01-01");
    },
    wizard3: async () => { await SCENARIOS.wizard2(); click("wz-next"); },
    wizard4: async () => { await SCENARIOS.wizard3(); await wait(50); click("wz-next"); },
    progress: async () => {
      await wait(200);
      const f = { folder: "Enviados", uid: 4411, reason: "imap error: injected: bad message uid 4411" };
      emit("progress", progress({ failed: 1, newFailures: [f] }));
      await wait(50);
      $("live-log-wrap").open = true;
    },
    migrate: async () => {
      await wait(200);
      emit("progress", progress({ op: "migrate", phase: "export", folder: "Projetos/2024", done: 77, total: 210, processed: 1200, foldersDone: 4 }));
    },
    partial: async () => {
      await wait(200);
      emit("finished", {
        profile: "joao-gmail", op: "archive", class: "partial", added: 112, duplicates: 18, failed: 3,
        failures: [
          { folder: "INBOX", uid: 9921, reason: "imap error: No Response: message too large" },
          { folder: "Enviados", uid: 4411, reason: "missing from fetch response" },
          { folder: "Pasta <script>alert(1)</script>", uid: null, reason: "folder not found" },
        ],
        failuresTotal: 3, error: null,
        phases: [{ phase: "archive", class: "partial", added: 112, duplicates: 18, failed: 3, error: null }],
        finishedAt: now,
      });
      await wait(100);
      $("result-details").open = true;
    },
    error: async () => {
      await wait(200);
      emit("finished", {
        profile: "joao-gmail", op: "archive", class: "error", added: 0, duplicates: 0, failed: 0,
        failures: [], failuresTotal: 0,
        error: "Não foi possível falar com o servidor. Confira o endereço, a porta, o tipo de segurança e a sua conexão com a internet.\nDetalhe técnico: connection error: failed to lookup address information: Este host não é conhecido.",
        phases: [{ phase: "archive", class: "error", added: 0, duplicates: 0, failed: 0, error: "x" }],
        finishedAt: now,
      });
    },
    ok: async () => {
      await wait(200);
      emit("finished", {
        profile: "joao-gmail", op: "migrate", class: "ok", added: 224, duplicates: 18000, failed: 0,
        failures: [], failuresTotal: 0, error: null,
        phases: [
          { phase: "archive", class: "ok", added: 112, duplicates: 0, failed: 0, error: null },
          { phase: "export", class: "ok", added: 112, duplicates: 18230, failed: 0, error: null },
        ],
        finishedAt: now,
      });
    },
    export: async () => { await wait(200); click("btn-export"); },
    "export-imap": async () => {
      await wait(200);
      document.querySelector('#account-list li:nth-child(2) button').click();
      await wait(100);
      click("btn-export");
      await wait(50);
    },
    remove: async () => { await wait(200); click("btn-remove"); },
  };
  window.addEventListener("load", () => setTimeout(() => (SCENARIOS[scenario] || (() => {}))(), 150));
  window.__mock = { emit, SCENARIOS };
})();
