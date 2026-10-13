// MailStore Backup Studio - Frontend JavaScript

let currentActiveTab = "tab-dashboard";
let currentExportTarget = "imap";
let activeViewerEmailId = null;
let lastSelectedExportProfile = "";
let customConfirmResolver = null;

// Modal Confirmação / Alerta Personalizado de Alta Fidelidade
function showCustomConfirm(title, message, confirmText = "Confirmar", cancelText = "Cancelar", icon = "⚠️") {
  return new Promise((resolve) => {
    customConfirmResolver = resolve;
    const titleElem = document.getElementById("confirm-modal-heading");
    const iconElem = document.getElementById("confirm-modal-icon");
    const msgElem = document.getElementById("confirm-modal-message");
    const btnOk = document.getElementById("confirm-btn-ok");
    const btnCancel = document.getElementById("confirm-btn-cancel");

    if (titleElem) titleElem.textContent = title;
    if (iconElem) iconElem.textContent = icon;
    if (msgElem) msgElem.textContent = message;
    if (btnOk) btnOk.textContent = confirmText;
    if (btnCancel) {
      btnCancel.textContent = cancelText || "Cancelar";
      btnCancel.style.display = cancelText ? "inline-flex" : "none";
    }
    const modal = document.getElementById("modal-confirm");
    if (modal) modal.classList.add("active");
  });
}

function showCustomAlert(title, message, icon = "ℹ️") {
  return showCustomConfirm(title, message, "Entendido", null, icon);
}

function closeCustomConfirmModal(result) {
  const modal = document.getElementById("modal-confirm");
  if (modal) modal.classList.remove("active");
  if (customConfirmResolver) {
    customConfirmResolver(result);
    customConfirmResolver = null;
  }
}

// Inicialização
document.addEventListener("DOMContentLoaded", () => {
  setupNavigation();
  loadSystemStatus();
  loadDashboardData();
  loadProfiles();
  loadFoldersTree();

  // Enter na busca
  const searchInput = document.getElementById("search-input");
  if (searchInput) {
    searchInput.addEventListener("keypress", (e) => {
      if (e.key === "Enter") executeSearch();
    });
  }
});

// Navegação entre abas
function setupNavigation() {
  document.querySelectorAll(".sidebar-nav .nav-link").forEach((link) => {
    link.addEventListener("click", () => {
      const targetTab = link.getAttribute("data-tab");
      navigateTo(targetTab);
    });
  });
}

function navigateTo(tabId) {
  document.querySelectorAll(".sidebar-nav .nav-link").forEach((l) => l.classList.remove("active"));
  const activeLink = document.querySelector(`.sidebar-nav .nav-link[data-tab="${tabId}"]`);
  if (activeLink) activeLink.classList.add("active");

  document.querySelectorAll(".tab-content").forEach((tab) => tab.classList.remove("active"));
  const targetElem = document.getElementById(tabId);
  if (targetElem) targetElem.classList.add("active");

  currentActiveTab = tabId;

  // Título da página
  const titles = {
    "tab-dashboard": "Início - Visão Geral do Arquivo",
    "tab-archive": "Arquivar E-mails - Perfis IMAP",
    "tab-search": "Pesquisar E-mails no Arquivo",
    "tab-browse": "Navegar no Arquivo - Pastas por Conta",
    "tab-export": "Exportar e Migrar E-mails",
    "tab-tools": "Ferramentas & Auditoria de Integridade",
  };
  document.getElementById("page-title").textContent = titles[tabId] || "Mailnov Backup";

  // Ações ao abrir abas
  if (tabId === "tab-dashboard") loadDashboardData();
  if (tabId === "tab-archive") loadProfiles();
  if (tabId === "tab-browse") loadFoldersTree();
  if (tabId === "tab-export") loadExportAccounts();
  if (tabId === "tab-tools") loadFullLogs();
}

// Formatadores
function formatBytes(bytes) {
  if (!bytes || bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatDate(isoStr) {
  if (!isoStr) return "-";
  try {
    const d = new Date(isoStr);
    return d.toLocaleString("pt-BR", { dateStyle: "short", timeStyle: "short" });
  } catch {
    return isoStr;
  }
}

// 1. Status e Dashboard
async function loadSystemStatus() {
  try {
    const res = await fetch("/api/status");
    const data = await res.json();
    const versionLabel = document.getElementById("app-version-label");
    if (versionLabel && data.version) versionLabel.textContent = `Versão ${data.version}`;
    const pstBadge = document.getElementById("dash-pst-badge");
    const sidebarPst = document.getElementById("sidebar-pst-status");

    if (data.pst_support && data.pst_support.available) {
      if (pstBadge) pstBadge.textContent = "Disponível (Nativo)";
      if (pstBadge) pstBadge.style.color = "var(--success)";
      if (sidebarPst) sidebarPst.textContent = "PST: Outlook COM Ativo";
    } else {
      if (pstBadge) pstBadge.textContent = "Indisponível";
      if (pstBadge) pstBadge.style.color = "var(--warning)";
      if (sidebarPst) sidebarPst.textContent = "PST: Requer Outlook";
    }
  } catch (err) {
    console.error("Erro ao carregar status:", err);
  }
}

async function loadDashboardData() {
  try {
    const res = await fetch("/api/status");
    const data = await res.json();

    document.getElementById("dash-total-msgs").textContent = (data.total_messages || 0).toLocaleString("pt-BR");
    document.getElementById("dash-total-size").textContent = formatBytes(data.total_size_bytes || 0);
    document.getElementById("dash-total-profiles").textContent = data.profiles_count || 0;
    document.getElementById("dash-folders-count").textContent = `${data.folders_count || 0} pastas indexadas`;

    // Logs recentes
    const logsRes = await fetch("/api/logs?limit=5");
    const logs = await logsRes.json();
    const tbody = document.getElementById("dash-logs-tbody");
    if (!logs || logs.length === 0) {
      tbody.innerHTML = '<tr><td colspan="6" style="text-align: center; color: var(--text-muted); padding: 20px;">Nenhuma sincronização recente.</td></tr>';
      return;
    }

    tbody.innerHTML = logs.map(l => `
      <tr>
        <td>${formatDate(l.started_at)}</td>
        <td><strong>${l.action.toUpperCase()}</strong></td>
        <td title="${escapeHtml(l.profile_username || l.profile_id)}">${escapeHtml(l.profile_name || l.profile_id)}</td>
        <td><span class="status-dot" style="background-color: ${l.status === 'success' ? 'var(--success)' : (l.status === 'error' ? 'var(--danger)' : 'var(--warning)')}"></span> ${l.status}</td>
        <td>${l.messages_count || 0}</td>
        <td>${l.errors_count || 0}</td>
      </tr>
    `).join("");
  } catch (err) {
    console.error("Erro ao carregar dashboard:", err);
  }
}

// 2. Perfis de Arquivamento IMAP
async function loadProfiles() {
  try {
    const res = await fetch("/api/profiles");
    const profiles = await res.json();
    const tbody = document.getElementById("profiles-tbody");

    if (!profiles || profiles.length === 0) {
      tbody.innerHTML = '<tr><td colspan="7" style="text-align: center; color: var(--text-muted); padding: 30px;">Nenhum perfil configurado. Clique no botão acima para adicionar sua primeira conta de e-mail.</td></tr>';
      return;
    }

    tbody.innerHTML = profiles.map(p => `
      <tr>
        <td><strong>${escapeHtml(p.name)}</strong></td>
        <td>${escapeHtml(p.imap.username)} <span style="color: var(--text-muted); font-size: 11px;">(${p.imap.host})</span></td>
        <td>${formatDate(p.last_run)}</td>
        <td>
          <span class="status-dot ${p.last_status === 'running' ? 'pulsing' : ''}" style="background-color: ${getStatusColor(p.last_status)}"></span>
          ${getStatusText(p.last_status)}
        </td>
        <td>${(p.message_count || 0).toLocaleString("pt-BR")}</td>
        <td>${formatBytes(p.total_size_bytes || 0)}</td>
        <td style="text-align: right;">
          <button class="btn btn-primary btn-sm" onclick="runBackupProfile('${p.id}')">▶ Arquivar</button>
          <button class="btn btn-secondary btn-sm" onclick="editProfile('${p.id}')">Editar</button>
          <button class="btn btn-danger btn-sm" onclick="deleteProfile('${p.id}')">Excluir</button>
        </td>
      </tr>
    `).join("");
  } catch (err) {
    console.error("Erro ao carregar perfis:", err);
  }
}

function getStatusColor(status) {
  if (status === "success") return "var(--success)";
  if (status === "running") return "var(--info)";
  if (status === "error") return "var(--danger)";
  if (status === "partial") return "var(--warning)";
  return "#94a3b8"; // idle
}

function getStatusText(status) {
  if (status === "success") return "Concluído";
  if (status === "running") return "Em execução...";
  if (status === "error") return "Erro";
  if (status === "partial") return "Com avisos";
  return "Nunca executado";
}

// Modal de Perfil
function openProfileModal(profile = null) {
  document.getElementById("prof-id").value = profile ? profile.id : "";
  document.getElementById("prof-name").value = profile ? profile.name : "";
  document.getElementById("prof-user").value = profile ? profile.imap.username : "";
  document.getElementById("prof-pass").value = profile ? profile.imap.password : "";
  document.getElementById("prof-host").value = profile ? profile.imap.host : "";
  document.getElementById("prof-port").value = profile ? profile.imap.port : "993";
  document.getElementById("prof-ssl").value = profile ? profile.imap.ssl_mode : "SSL";
  document.getElementById("prof-since-date").value = profile ? (profile.since_date || "") : "";
  document.getElementById("test-conn-feedback").textContent = "";
  document.getElementById("prof-folders-checklist").innerHTML =
    `<span style="color: var(--text-muted);">Clique em 'Testar Conexão' acima para listar as pastas disponíveis no servidor.</span>`;

  document.getElementById("modal-profile-title").textContent = profile ? "Editar Perfil IMAP" : "Adicionar Novo Perfil IMAP";
  document.getElementById("modal-profile").classList.add("active");
}

function closeProfileModal() {
  document.getElementById("modal-profile").classList.remove("active");
}

async function autoDetectSettings() {
  const emailVal = document.getElementById("prof-user").value.trim();
  const hostInput = document.getElementById("prof-host");
  if (!emailVal || hostInput.value.trim() !== "") return;

  try {
    const res = await fetch(`/api/detect-provider?email=${encodeURIComponent(emailVal)}`);
    const data = await res.json();
    if (data.detected) {
      hostInput.value = data.detected.host;
      document.getElementById("prof-port").value = data.detected.port;
      document.getElementById("prof-ssl").value = data.detected.ssl_mode;
      const feedback = document.getElementById("test-conn-feedback");
      feedback.style.color = "var(--info)";
      feedback.textContent = `Configuração sugerida para ${data.detected.name}`;
    }
  } catch (err) {
    console.error(err);
  }
}

async function testCurrentConnection() {
  const feedback = document.getElementById("test-conn-feedback");
  feedback.style.color = "var(--text-muted)";
  feedback.textContent = "Testando conexão IMAP...";

  const payload = {
    host: document.getElementById("prof-host").value.trim(),
    port: parseInt(document.getElementById("prof-port").value) || 993,
    ssl_mode: document.getElementById("prof-ssl").value,
    username: document.getElementById("prof-user").value.trim(),
    password: document.getElementById("prof-pass").value
  };

  try {
    const res = await fetch("/api/test-connection", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload)
    });
    const result = await res.json();

    if (result.success) {
      feedback.style.color = "var(--success)";
      feedback.textContent = `✓ Conexão bem-sucedida! ${result.folders_count} pasta(s) localizada(s).`;

      // Preenche lista de pastas
      const listDiv = document.getElementById("prof-folders-checklist");
      listDiv.innerHTML = result.folders.map(f => `
        <label style="display: block; margin-bottom: 4px; cursor: pointer;">
          <input type="checkbox" name="prof_folder" value="${escapeHtml(f)}" checked>
          📁 ${escapeHtml(f)}
        </label>
      `).join("");
    } else {
      feedback.style.color = "var(--danger)";
      feedback.textContent = `✗ Falha: ${result.message}`;
    }
  } catch (err) {
    feedback.style.color = "var(--danger)";
    feedback.textContent = `Erro ao testar: ${err.message}`;
  }
}

async function saveProfileAndOptionallyRun(runImmediately = false) {
  const profId = document.getElementById("prof-id").value;
  const name = document.getElementById("prof-name").value.trim() || document.getElementById("prof-user").value.trim();
  const user = document.getElementById("prof-user").value.trim();
  const pass = document.getElementById("prof-pass").value;
  const host = document.getElementById("prof-host").value.trim();
  const port = parseInt(document.getElementById("prof-port").value) || 993;
  const ssl_mode = document.getElementById("prof-ssl").value;
  const since_date = document.getElementById("prof-since-date").value || null;

  if (!user || !pass || !host) {
    await showCustomAlert("Campos Obrigatórios", "Por favor, preencha o e-mail, senha e servidor IMAP.", "⚠️");
    return;
  }

  // Pastas selecionadas
  const selectedFolders = [];
  document.querySelectorAll('input[name="prof_folder"]:checked').forEach(cb => selectedFolders.push(cb.value));

  const payload = {
    name,
    imap: { host, port, ssl_mode, username: user, password: pass },
    selected_folders: selectedFolders,
    since_date
  };

  try {
    let savedProfile;
    if (profId) {
      const res = await fetch(`/api/profiles/${profId}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      savedProfile = await res.json();
    } else {
      const res = await fetch("/api/profiles", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload)
      });
      savedProfile = await res.json();
    }

    closeProfileModal();
    loadProfiles();
    loadDashboardData();

    if (runImmediately && savedProfile && savedProfile.id) {
      runBackupProfile(savedProfile.id);
    }
  } catch (err) {
    await showCustomAlert("Erro ao Salvar", "Erro ao salvar perfil: " + err.message, "❌");
  }
}

async function editProfile(id) {
  const res = await fetch(`/api/profiles/${id}`);
  const profile = await res.json();
  openProfileModal(profile);
}

async function deleteProfile(id) {
  const confirmed = await showCustomConfirm(
    "Excluir Perfil de Backup",
    "Deseja realmente remover este perfil? O índice de busca e configuração desta conta serão excluídos do sistema.",
    "Sim, excluir",
    "Cancelar",
    "🗑️"
  );
  if (!confirmed) return;
  await fetch(`/api/profiles/${id}`, { method: "DELETE" });
  loadProfiles();
  loadDashboardData();
}

// 3. Execução e Acompanhamento de Backup
async function runBackupProfile(profileId) {
  try {
    const res = await fetch(`/api/profiles/${profileId}/backup`, { method: "POST" });
    if (!res.ok) {
      const err = await res.json();
      await showCustomAlert("Erro no Backup", "Erro ao iniciar backup: " + (err.detail || "Falha na conexão"), "❌");
      return;
    }
    let name = profileId;
    try { name = (await (await fetch(`/api/profiles/${profileId}`)).json()).name || name; } catch (e) {}
    openProgressModal(`Backup: ${name}`, "backup", profileId);
  } catch (err) {
    await showCustomAlert("Erro de Comunicação", "Erro de comunicação: " + err.message, "❌");
  }
}

// Registro de jobs em segundo plano: vários podem rodar em paralelo.
const backgroundJobs = {};
let viewedJobKey = null;

function jobKeyOf(jobType, targetId) {
  return jobType === "backup" ? `backup:${targetId}` : "export";
}

// Inicia o acompanhamento minimizado (chip no dock); o log só abre ao clicar.
function openProgressModal(title, jobType, targetId) {
  const key = jobKeyOf(jobType, targetId);
  if (backgroundJobs[key] && backgroundJobs[key].poller) clearInterval(backgroundJobs[key].poller);
  const job = { key, title, jobType, targetId, data: { percent: 0, status: "running", logs: [] }, poller: null };
  backgroundJobs[key] = job;
  job.poller = setInterval(() => pollJob(job), 1000);
  pollJob(job);
  renderJobDock();
}

async function pollJob(job) {
  try {
    const url = job.jobType === "backup" ? `/api/profiles/${job.targetId}/progress` : "/api/jobs/export/progress";
    const res = await fetch(url);
    job.data = await res.json();
  } catch (err) {
    console.error(err);
    return;
  }
  if (job.data.status !== "running" && job.poller) {
    clearInterval(job.poller);
    job.poller = null;
    loadDashboardData();
    loadProfiles();
    loadFoldersTree();
  }
  renderJobDock();
  if (viewedJobKey === job.key) renderProgressModal(job);
}

function renderJobDock() {
  const dock = document.getElementById("job-dock");
  if (!dock) return;
  const icons = { running: "⏳", success: "✅", partial: "⚠️", warning: "⚠️", cancelled: "⏹️", error: "❌" };
  dock.innerHTML = Object.values(backgroundJobs).map(job => {
    const d = job.data;
    const pct = d.percent || 0;
    const running = d.status === "running";
    return `<div class="job-chip ${running ? "" : "job-done"}" onclick="viewJob('${job.key}')" title="Clique para ver o log">
      <div class="job-chip-top"><span>${icons[d.status] || "⏳"} ${escapeHtml(job.title)}</span><b>${pct}%</b>
      ${running ? "" : `<span class="job-chip-x" onclick="event.stopPropagation(); dismissJob('${job.key}')">&times;</span>`}</div>
      <div class="job-chip-bar"><div style="width:${pct}%"></div></div></div>`;
  }).join("");
}

function dismissJob(key) {
  delete backgroundJobs[key];
  if (viewedJobKey === key) minimizeProgressModal();
  renderJobDock();
}

function viewJob(key) {
  const job = backgroundJobs[key];
  if (!job) return;
  viewedJobKey = key;
  document.getElementById("progress-modal-title").innerHTML =
    `<span class="modal-icon">${job.jobType === "backup" ? "📥" : "🚀"}</span> <span>${escapeHtml(job.title)}</span>`;
  document.getElementById("modal-progress").classList.add("active");
  renderProgressModal(job);
}

function renderProgressModal(job) {
  const data = job.data;
  const set = (id, v) => { document.getElementById(id).textContent = v; };
  const pct = data.percent || 0;
  document.getElementById("progress-bar-fill").style.width = `${pct}%`;
  set("progress-percent-label", `${pct}%`);
  let cur, tot;
  if (job.jobType === "backup") {
    cur = data.current_msg || 0; tot = data.total_msgs || 0;
    set("progress-details-current", `Pasta: ${data.current_folder || "-"} (${cur}/${tot})`);
    set("progress-speed", `${data.speed_mps || 0} msgs/s`);
    set("prog-stat-success", data.synced_count || 0);
  } else {
    cur = data.current_index || 0; tot = data.total_messages || 0;
    set("progress-details-current", `Progresso: ${cur}/${tot}`);
    set("progress-speed", data.current_subject ? data.current_subject.substring(0, 30) : "");
    set("prog-stat-success", data.success_count || 0);
  }
  set("prog-stat-processed", `${cur} / ${tot}`);
  set("prog-stat-errors", data.error_count || 0);

  const term = document.getElementById("progress-log-terminal");
  if (data.logs) {
    term.innerHTML = data.logs.map(escapeHtml).join("<br>");
    term.scrollTop = term.scrollHeight;
  }

  const running = data.status === "running";
  document.getElementById("btn-cancel-job").style.display = running ? "inline-flex" : "none";
  document.getElementById("btn-close-progress").style.display = running ? "none" : "inline-flex";
  const el = document.getElementById("progress-status-desc");
  if (running) {
    el.textContent = job.cancelRequested ? "Cancelamento solicitado..." : "Processando mensagens...";
  } else if (data.status === "success") {
    const count = data.success_count !== undefined ? data.success_count : (data.total_messages || data.synced_count || 0);
    el.innerHTML = `<span style="color: var(--success); font-weight: 600;">✅ Concluído com sucesso! (${count} e-mails processados)</span>`;
  } else if (data.status === "partial") {
    el.innerHTML = `<span style="color: #d97706; font-weight: 600;">⚠️ Concluído com avisos: ${data.success_count ?? data.synced_count ?? 0} processados, ${data.error_count || 0} falhas.</span>`;
  } else if (data.status === "warning") {
    el.innerHTML = `<span style="color: #d97706; font-weight: 600;">⚠️ Atenção: Nenhuma mensagem encontrada com os filtros selecionados.</span>`;
  } else if (data.status === "cancelled") {
    el.innerHTML = `<span style="color: var(--text-muted); font-weight: 600;">⏹️ Operação cancelada pelo usuário.</span>`;
  } else {
    el.innerHTML = `<span style="color: var(--danger); font-weight: 600;">❌ Falha na operação (${data.error_count || 0} erros).</span>`;
  }
}

// Esconde o modal; o job continua rodando e aparece no dock.
function minimizeProgressModal() {
  viewedJobKey = null;
  document.getElementById("modal-progress").classList.remove("active");
}

// "Fechar" após terminar: minimiza e remove o chip concluído.
function closeProgressModal() {
  const key = viewedJobKey;
  minimizeProgressModal();
  if (key && backgroundJobs[key] && backgroundJobs[key].data.status !== "running") dismissJob(key);
}

async function cancelCurrentJob() {
  const job = backgroundJobs[viewedJobKey];
  if (!job) return;
  const url = job.jobType === "backup" ? `/api/profiles/${job.targetId}/cancel` : "/api/jobs/export/cancel";
  try {
    const res = await fetch(url, { method: "POST" });
    if (res.ok) job.cancelRequested = true;
  } catch (err) { console.error(err); }
  renderProgressModal(job);
}

// 4. Pesquisar E-mails
function toggleAdvancedSearch() {
  const adv = document.getElementById("search-advanced-filters");
  adv.style.display = adv.style.display === "none" ? "block" : "none";
}

async function executeSearch() {
  const query = document.getElementById("search-input").value.trim();
  const accountId = document.getElementById("filter-account") ? document.getElementById("filter-account").value : null;
  const sender = document.getElementById("filter-sender").value.trim();
  const recipient = document.getElementById("filter-recipient").value.trim();
  const subject = document.getElementById("filter-subject").value.trim();
  const dateFrom = document.getElementById("filter-date-from").value || null;
  const dateTo = document.getElementById("filter-date-to").value || null;
  const hasAtt = document.getElementById("filter-has-attachment").checked ? true : null;

  const payload = {
    query,
    account_id: accountId || null,
    sender: sender || null,
    recipient: recipient || null,
    subject: subject || null,
    date_from: dateFrom,
    date_to: dateTo,
    has_attachment: hasAtt,
    limit: 100
  };

  const tbody = document.getElementById("search-results-tbody");
  tbody.innerHTML = '<tr><td colspan="7" style="text-align: center; color: var(--text-muted); padding: 30px;">Pesquisando...</td></tr>';

  try {
    const res = await fetch("/api/search", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload)
    });
    const data = await res.json();

    document.getElementById("search-results-info").textContent = `${data.total} mensagem(ns) encontrada(s).`;

    if (!data.items || data.items.length === 0) {
      tbody.innerHTML = '<tr><td colspan="7" style="text-align: center; color: var(--text-muted); padding: 30px;">Nenhuma mensagem encontrada com os filtros informados.</td></tr>';
      return;
    }

    tbody.innerHTML = data.items.map(m => `
      <tr onclick="openEmailViewer(${m.id})">
        <td style="text-align: center;">${m.has_attachments ? '📎' : ''}</td>
        <td>${formatDate(m.date_sent)}</td>
        <td><strong>${escapeHtml(m.sender)}</strong></td>
        <td>${escapeHtml(m.recipients)}</td>
        <td>${escapeHtml(m.subject)}</td>
        <td><span class="badge-count">${escapeHtml(m.folder)}</span></td>
        <td>${formatBytes(m.size_bytes)}</td>
      </tr>
    `).join("");
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="7" style="color: var(--danger); text-align: center;">Erro na pesquisa: ${err.message}</td></tr>`;
  }
}

// 5. Navegar no Arquivo (Árvore Separada por Perfil)
async function loadFoldersTree() {
  try {
    const res = await fetch("/api/browse/tree");
    const tree = await res.json();
    const treeDiv = document.getElementById("folders-tree-list");

    if (!tree || tree.length === 0) {
      treeDiv.innerHTML = '<div style="color: var(--text-muted); font-size: 12px; padding: 12px;">Nenhuma conta ou pasta arquivada ainda.</div>';
      return;
    }

    // Atualiza filtro de contas na busca
    updateSearchAccountFilter(tree);

    let html = "";
    tree.forEach((account) => {
      const hasFolders = account.folders && account.folders.length > 0;
      const totalMsgs = account.total_messages || 0;

      html += `
        <div class="profile-tree-group" id="acc-group-${account.account_id}">
          <div class="profile-tree-header" onclick="selectBrowseProfile('${escapeHtml(account.account_id)}', '${escapeHtml(account.account_name)}', '${escapeHtml(account.username)}')">
            <span class="profile-toggle-btn collapsed" id="toggle-btn-${account.account_id}" onclick="event.stopPropagation(); toggleProfileFolders('${escapeHtml(account.account_id)}')">▶</span>
            <span style="font-size: 15px;">📬</span>
            <div class="profile-info">
              <div class="profile-name" title="${escapeHtml(account.account_name)}">${escapeHtml(account.account_name)}</div>
              <div class="profile-sub" title="${escapeHtml(account.username)}">${escapeHtml(account.username)}</div>
            </div>
            <span class="badge-count" title="Total na conta: ${totalMsgs}">${totalMsgs}</span>
          </div>

          <div class="profile-folders-container collapsed" id="folders-container-${account.account_id}">
            ${
              hasFolders
                ? account.folders
                    .map(
                      (f) => `
              <div class="tree-node sub-folder" id="folder-node-${account.account_id}-${escapeHtml(f.folder_name)}" onclick="event.stopPropagation(); selectBrowseFolder('${escapeHtml(account.account_id)}', '${escapeHtml(account.account_name)}', '${escapeHtml(f.folder_name)}')">
                <span title="${escapeHtml(f.folder_name)}">📁 ${escapeHtml(f.folder_name)}</span>
                <span class="badge-count">${f.message_count}</span>
              </div>
            `
                    )
                    .join("")
                : '<div style="font-size: 11px; color: var(--text-muted); padding: 6px 12px;">Nenhuma pasta arquivada</div>'
            }
          </div>
        </div>
      `;
    });

    treeDiv.innerHTML = html;
  } catch (err) {
    console.error("Erro ao carregar árvore por perfil:", err);
  }
}

function updateSearchAccountFilter(tree) {
  const sel = document.getElementById("filter-account");
  if (!sel) return;
  const currentVal = sel.value;
  sel.innerHTML = '<option value="">Todas as contas</option>' +
    tree.map(a => `<option value="${escapeHtml(a.account_id)}">${escapeHtml(a.account_name)} (${escapeHtml(a.username)})</option>`).join("");
  if (currentVal) sel.value = currentVal;
}

function toggleProfileFolders(accountId) {
  const container = document.getElementById(`folders-container-${accountId}`);
  const btn = document.getElementById(`toggle-btn-${accountId}`);
  if (container) {
    const isCollapsed = container.classList.toggle("collapsed");
    if (btn) {
      btn.textContent = isCollapsed ? "▶" : "▼";
      btn.classList.toggle("collapsed", isCollapsed);
    }
  }
}

function clearTreeActiveSelection() {
  document.querySelectorAll(".profile-tree-header, .tree-node").forEach(n => n.classList.remove("active"));
}

async function selectBrowseProfile(accountId, accountName, username) {
  clearTreeActiveSelection();
  const headerElem = document.querySelector(`#acc-group-${accountId} .profile-tree-header`);
  if (headerElem) headerElem.classList.add("active");

  const badge = document.getElementById("browse-account-badge");
  if (badge) {
    badge.style.display = "inline-block";
    badge.textContent = accountName;
  }

  const titleElem = document.getElementById("browse-folder-title");
  if (titleElem) titleElem.textContent = `Todas as pastas (${username})`;

  const tbody = document.getElementById("browse-messages-tbody");
  tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">Carregando mensagens da conta...</td></tr>';

  try {
    const res = await fetch(`/api/messages?account_id=${encodeURIComponent(accountId)}&limit=100`);
    const data = await res.json();

    document.getElementById("browse-folder-count").textContent = `${data.total} mensagens`;

    if (!data.items || data.items.length === 0) {
      tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">Esta conta ainda não possui mensagens arquivadas.</td></tr>';
      return;
    }

    renderBrowseMessages(data.items);
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="5" style="color: var(--danger); text-align: center;">Erro: ${err.message}</td></tr>`;
  }
}

async function selectBrowseFolder(accountId, accountName, folderName) {
  clearTreeActiveSelection();
  const node = document.getElementById(`folder-node-${accountId}-${folderName}`);
  if (node) node.classList.add("active");

  const badge = document.getElementById("browse-account-badge");
  if (badge) {
    badge.style.display = "inline-block";
    badge.textContent = accountName;
  }

  const titleElem = document.getElementById("browse-folder-title");
  if (titleElem) titleElem.textContent = `📁 ${folderName}`;

  const tbody = document.getElementById("browse-messages-tbody");
  tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">Carregando mensagens da pasta...</td></tr>';

  try {
    const res = await fetch(`/api/messages?account_id=${encodeURIComponent(accountId)}&folder=${encodeURIComponent(folderName)}&limit=100`);
    const data = await res.json();

    document.getElementById("browse-folder-count").textContent = `${data.total} mensagens`;

    if (!data.items || data.items.length === 0) {
      tbody.innerHTML = '<tr><td colspan="5" style="text-align: center; color: var(--text-muted); padding: 30px;">Esta pasta está vazia.</td></tr>';
      return;
    }

    renderBrowseMessages(data.items);
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="5" style="color: var(--danger); text-align: center;">Erro: ${err.message}</td></tr>`;
  }
}

function renderBrowseMessages(items) {
  const tbody = document.getElementById("browse-messages-tbody");
  tbody.innerHTML = items.map(m => `
    <tr onclick="openEmailViewer(${m.id})">
      <td style="text-align: center;">${m.has_attachments ? '📎' : ''}</td>
      <td>${formatDate(m.date_sent)}</td>
      <td><strong>${escapeHtml(m.sender)}</strong></td>
      <td>${escapeHtml(m.subject)}</td>
      <td>${formatBytes(m.size_bytes)}</td>
    </tr>
  `).join("");
}

// 6. Visualizador de Mensagem
async function openEmailViewer(messageId) {
  activeViewerEmailId = messageId;
  const modal = document.getElementById("modal-viewer");
  modal.classList.add("active");

  document.getElementById("viewer-subject-title").textContent = "Carregando mensagem...";
  document.getElementById("viewer-subject").textContent = "";
  document.getElementById("viewer-from").textContent = "";
  document.getElementById("viewer-to").textContent = "";
  document.getElementById("viewer-date").textContent = "";
  document.getElementById("viewer-folder").textContent = "";
  document.getElementById("viewer-attachments-box").style.display = "none";

  try {
    const res = await fetch(`/api/messages/${messageId}`);
    const emailData = await res.json();

    document.getElementById("viewer-subject-title").textContent = emailData.subject;
    document.getElementById("viewer-subject").textContent = emailData.subject;
    document.getElementById("viewer-from").textContent = emailData.sender;
    document.getElementById("viewer-to").textContent = emailData.recipients;
    document.getElementById("viewer-date").textContent = formatDate(emailData.date_sent);
    document.getElementById("viewer-folder").textContent = emailData.folder;

    // Cc
    const ccRow = document.getElementById("viewer-cc-row");
    if (emailData.cc) {
      ccRow.style.display = "block";
      document.getElementById("viewer-cc").textContent = emailData.cc;
    } else {
      ccRow.style.display = "none";
    }

    // Anexos
    const attBox = document.getElementById("viewer-attachments-box");
    if (emailData.attachments && emailData.attachments.length > 0) {
      attBox.style.display = "flex";
      attBox.innerHTML = emailData.attachments.map(att => `
        <a class="attachment-pill" href="/api/messages/${emailData.id}/attachments/${att.part_index}" target="_blank">
          📎 ${escapeHtml(att.filename)} (${formatBytes(att.size_bytes)})
        </a>
      `).join("");
    } else {
      attBox.style.display = "none";
    }

    // Corpos
    const iframe = document.getElementById("viewer-html-frame");
    const pre = document.getElementById("viewer-text-pre");

    if (emailData.body_html) {
      iframe.srcdoc = emailData.body_html;
      iframe.style.display = "block";
      pre.style.display = "none";
      switchViewerTab("html");
    } else {
      pre.textContent = emailData.body_text || emailData.body_preview || "(Mensagem sem corpo de texto)";
      pre.style.display = "block";
      iframe.style.display = "none";
      switchViewerTab("text");
    }

    // Botão baixar EML
    document.getElementById("btn-download-eml").onclick = () => {
      window.location.href = `/api/messages/${emailData.id}/eml`;
    };
  } catch (err) {
    await showCustomAlert("Erro na Mensagem", "Erro ao carregar detalhes da mensagem: " + err.message, "❌");
  }
}

function switchViewerTab(tab) {
  const tabBtnHtml = document.getElementById("tab-btn-html");
  const tabBtnText = document.getElementById("tab-btn-text");
  const iframe = document.getElementById("viewer-html-frame");
  const pre = document.getElementById("viewer-text-pre");

  if (tab === "html") {
    tabBtnHtml.classList.add("active");
    tabBtnText.classList.remove("active");
    iframe.style.display = "block";
    pre.style.display = "none";
  } else {
    tabBtnText.classList.add("active");
    tabBtnHtml.classList.remove("active");
    iframe.style.display = "none";
    pre.style.display = "block";
  }
}

function closeViewerModal() {
  document.getElementById("modal-viewer").classList.remove("active");
}

// 7. Exportação e Migração
function selectExportTarget(target) {
  currentExportTarget = target;
  document.querySelectorAll(".export-card").forEach(c => c.classList.remove("selected"));
  document.getElementById(`card-exp-${target}`).classList.add("selected");

  document.getElementById("export-fields-imap").style.display = target === "imap" ? "block" : "none";
  document.getElementById("export-fields-pst").style.display = target === "pst" ? "block" : "none";
  document.getElementById("export-fields-eml").style.display = target === "eml" ? "block" : "none";
  document.getElementById("export-fields-mbox").style.display = target === "mbox" ? "block" : "none";
}

// --- Explorador de Pastas e Arquivos Integrado ---
let fsPickerMode = "file"; // "file" ou "dir"
let fsPickerTargetInputId = "";
let fsPickerCurrentPath = "";
let fsPickerParentPath = "";

function openFsPicker(targetInputId, mode, defaultName, title) {
  fsPickerTargetInputId = targetInputId;
  fsPickerMode = mode;

  const modalIcon = document.getElementById("fs-modal-icon");
  const modalTitle = document.getElementById("fs-modal-title");
  const filenameRow = document.getElementById("fs-filename-row");
  const filenameInput = document.getElementById("fs-input-filename");

  if (modalTitle) modalTitle.textContent = title;
  if (modalIcon) modalIcon.textContent = mode === "dir" ? "📁" : "📑";

  if (mode === "file") {
    if (filenameRow) filenameRow.style.display = "block";
    const currentVal = document.getElementById(targetInputId) ? document.getElementById(targetInputId).value.trim() : "";
    let nameToUse = defaultName;
    if (currentVal) {
      const parts = currentVal.split(/[\\/]/);
      if (parts.length > 0 && parts[parts.length - 1].includes(".")) {
        nameToUse = parts[parts.length - 1];
      }
    }
    if (filenameInput) filenameInput.value = nameToUse;
  } else {
    if (filenameRow) filenameRow.style.display = "none";
  }

  // Caminho inicial: se o campo já tiver algo, começa por lá
  let initialPath = "";
  const existingVal = document.getElementById(targetInputId) ? document.getElementById(targetInputId).value.trim() : "";
  if (existingVal) {
    if (mode === "file") {
      const idx = Math.max(existingVal.lastIndexOf("\\"), existingVal.lastIndexOf("/"));
      initialPath = idx > 0 ? existingVal.substring(0, idx) : "";
    } else {
      initialPath = existingVal;
    }
  }

  const modal = document.getElementById("modal-fs-picker");
  if (modal) modal.classList.add("active");

  loadFsPath(initialPath);
}

async function loadFsPath(path = "") {
  const container = document.getElementById("fs-folder-list");
  if (container) {
    container.innerHTML = '<div style="color: var(--text-muted); font-size: 12px; padding: 30px; text-align: center;">Carregando diretórios...</div>';
  }

  try {
    const res = await fetch(`/api/fs/browse?path=${encodeURIComponent(path)}`);
    const data = await res.json();

    fsPickerCurrentPath = data.current_path || "";
    fsPickerParentPath = data.parent_path || "";

    const pathInput = document.getElementById("fs-input-path");
    if (pathInput) pathInput.value = fsPickerCurrentPath;

    const btnUp = document.getElementById("fs-btn-up");
    if (btnUp) btnUp.disabled = !fsPickerParentPath;

    // Renderiza Atalhos e Unidades
    const shortcutsDiv = document.getElementById("fs-shortcuts-container");
    if (shortcutsDiv) {
      let shortcutsHtml = "";
      if (data.shortcuts && data.shortcuts.length > 0) {
        data.shortcuts.forEach(s => {
          shortcutsHtml += `<button type="button" class="fs-shortcut-btn" onclick="loadFsPath('${escapeHtml(s.path.replace(/\\/g, '\\\\'))}')">${s.icon} ${escapeHtml(s.name)}</button>`;
        });
      }
      if (data.drives && data.drives.length > 0) {
        data.drives.forEach(d => {
          shortcutsHtml += `<button type="button" class="fs-shortcut-btn" onclick="loadFsPath('${escapeHtml(d.replace(/\\/g, '\\\\'))}')">💾 ${escapeHtml(d)}</button>`;
        });
      }
      shortcutsDiv.innerHTML = shortcutsHtml;
    }

    // Renderiza Lista de Pastas
    if (container) {
      if (!data.directories || data.directories.length === 0) {
        container.innerHTML = '<div style="color: var(--text-muted); font-size: 12px; padding: 25px; text-align: center;">Nenhuma subpasta encontrada nesta pasta.</div>';
      } else {
        container.innerHTML = data.directories.map(d => `
          <div class="fs-folder-item" onclick="loadFsPath('${escapeHtml(d.path.replace(/\\/g, '\\\\'))}')" title="${escapeHtml(d.name)}">
            <span>📁</span>
            <span style="flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escapeHtml(d.name)}</span>
          </div>
        `).join("");
      }
    }

    updateFsPreview();
  } catch (err) {
    if (container) {
      container.innerHTML = `<div style="color: var(--danger); font-size: 12px; padding: 20px; text-align: center;">Erro ao ler diretório: ${escapeHtml(err.message)}</div>`;
    }
  }
}

function updateFsPreview() {
  const previewElem = document.getElementById("fs-preview-full-path");
  if (!previewElem) return;

  if (fsPickerMode === "file") {
    const filenameInput = document.getElementById("fs-input-filename");
    const filename = filenameInput ? filenameInput.value.trim() : "backup_emails.pst";
    const cleanCurrent = fsPickerCurrentPath.replace(/[\/\\]+$/, "");
    const full = cleanCurrent ? `${cleanCurrent}\\${filename}` : filename;
    previewElem.textContent = full;
  } else {
    previewElem.textContent = fsPickerCurrentPath;
  }
}

function fsNavigateUp() {
  if (fsPickerParentPath) {
    loadFsPath(fsPickerParentPath);
  }
}

function fsNavigateToPath(customPath) {
  if (customPath && customPath.trim()) {
    loadFsPath(customPath.trim());
  }
}

async function fsCreateNewFolder() {
  const folderName = prompt("Digite o nome da nova pasta:");
  if (!folderName || !folderName.trim()) return;

  try {
    const res = await fetch("/api/fs/create-folder", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        parent_path: fsPickerCurrentPath,
        folder_name: folderName.trim()
      })
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.detail || "Falha ao criar pasta");
    loadFsPath(data.created_path);
  } catch (err) {
    alert("Não foi possível criar a pasta: " + err.message);
  }
}

function confirmFsSelection() {
  const previewElem = document.getElementById("fs-preview-full-path");
  if (previewElem && previewElem.textContent && fsPickerTargetInputId) {
    const targetInput = document.getElementById(fsPickerTargetInputId);
    if (targetInput) {
      targetInput.value = previewElem.textContent;
    }
  }
  closeFsPickerModal();
}

function closeFsPickerModal() {
  const modal = document.getElementById("modal-fs-picker");
  if (modal) modal.classList.remove("active");
}

function browseSavePstPath() {
  openFsPicker("dest-pst-path", "file", "backup_emails.pst", "Salvar Arquivo PST do Outlook");
}

function browseSelectEmlDir() {
  openFsPicker("dest-eml-dir", "dir", "", "Selecionar Pasta de Destino para EMLs");
}

function browseSaveMboxPath() {
  openFsPicker("dest-mbox-path", "file", "emails.mbox", "Salvar Arquivo MBOX");
}

async function loadExportAccounts() {
  try {
    const res = await fetch("/api/profiles");
    const profiles = await res.json();
    const sel = document.getElementById("export-source-profile");
    let optionsHtml = '<option value="">-- Selecione uma conta --</option>';
    profiles.forEach(p => {
      optionsHtml += `<option value="${p.id}">👤 ${escapeHtml(p.name)} (${p.imap.username})</option>`;
    });
    optionsHtml += '<option value="__all__">📁 Todas as contas arquivadas</option>';
    sel.innerHTML = optionsHtml;
    lastSelectedExportProfile = "";
    loadFoldersForExport();
  } catch (err) {
    console.error(err);
  }
}

async function handleExportProfileChange() {
  const sel = document.getElementById("export-source-profile");
  const val = sel.value;
  if (val === "__all__") {
    const confirmed = await showCustomConfirm(
      "Atenção: Exportar Todas as Contas",
      "Tem certeza de que deseja selecionar TODAS as contas arquivadas para exportação?\n\nEsta operação reunirá o histórico completo de e-mails de múltiplos perfis e enviará para o mesmo destino.",
      "Sim, tenho certeza",
      "Cancelar",
      "⚠️"
    );
    if (!confirmed) {
      sel.value = lastSelectedExportProfile;
      loadFoldersForExport();
      return;
    }
  }
  lastSelectedExportProfile = sel.value;
  loadFoldersForExport();
}

async function loadFoldersForExport() {
  const profileVal = document.getElementById("export-source-profile").value;
  const sel = document.getElementById("export-source-folder");
  if (!profileVal || profileVal === "__all__") {
    sel.innerHTML = '<option value="">Todas as pastas</option>';
    sel.disabled = (profileVal === "__all__");
    return;
  }
  sel.disabled = false;
  try {
    const res = await fetch(`/api/folders?account_id=${encodeURIComponent(profileVal)}`);
    const folders = await res.json();
    sel.innerHTML = '<option value="">Todas as pastas</option>' +
      folders.map(f => `<option value="${escapeHtml(f.folder_name)}">${escapeHtml(f.folder_name)} (${f.message_count})</option>`).join("");
  } catch (err) {
    console.error(err);
  }
}

async function startExport() {
  const profileVal = document.getElementById("export-source-profile").value;
  if (!profileVal) {
    await showCustomAlert(
      "Conta Não Selecionada",
      "Por favor, selecione uma conta de origem ou a opção 'Todas as contas arquivadas' antes de iniciar.",
      "⚠️"
    );
    return;
  }

  const profileId = (profileVal === "__all__") ? null : profileVal;
  const folder = (profileVal === "__all__") ? null : (document.getElementById("export-source-folder").value || null);

  if (profileVal === "__all__") {
    const confirmed = await showCustomConfirm(
      "Confirmação de Exportação Global",
      "Você está prestes a exportar o acervo de TODAS as contas cadastradas simultaneamente.\n\nDeseja prosseguir?",
      "Iniciar Exportação",
      "Cancelar",
      "🚀"
    );
    if (!confirmed) return;
  }

  try {
    if (currentExportTarget === "imap") {
      const host = document.getElementById("dest-imap-host").value.trim();
      const port = parseInt(document.getElementById("dest-imap-port").value) || 993;
      const ssl = document.getElementById("dest-imap-ssl").value;
      const user = document.getElementById("dest-imap-user").value.trim();
      const pass = document.getElementById("dest-imap-pass").value;
      const prefix = document.getElementById("dest-imap-prefix").value.trim();

      if (!host || !user || !pass) {
        await showCustomAlert("Campos Obrigatórios", "Preencha o servidor, usuário e senha do IMAP destino.", "⚠️");
        return;
      }

      const res = await fetch("/api/export/imap", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          profile_id: profileId,
          folder: folder,
          target_host: host,
          target_port: port,
          target_ssl_mode: ssl,
          target_username: user,
          target_password: pass,
          target_folder_prefix: prefix
        })
      });
      if (!res.ok) throw new Error((await res.json()).detail);
      openProgressModal("Migração IMAP em Andamento", "export", null);

    } else if (currentExportTarget === "pst") {
      const outPath = document.getElementById("dest-pst-path").value.trim();
      if (!outPath) {
        await showCustomAlert("Caminho Obrigatório", "Informe o caminho do arquivo .pst de saída.", "⚠️");
        return;
      }

      const res = await fetch("/api/export/pst", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ profile_id: profileId, folder: folder, output_path: outPath })
      });
      if (!res.ok) throw new Error((await res.json()).detail);
      openProgressModal("Exportação PST do Outlook em Andamento", "export", null);

    } else if (currentExportTarget === "eml") {
      const outDir = document.getElementById("dest-eml-dir").value.trim();
      if (!outDir) {
        await showCustomAlert("Diretório Obrigatório", "Informe o diretório de destino para os arquivos .EML.", "⚠️");
        return;
      }

      const res = await fetch("/api/export/eml", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ profile_id: profileId, folder: folder, output_dir: outDir })
      });
      if (!res.ok) throw new Error((await res.json()).detail);
      openProgressModal("Exportação EML em Andamento", "export", null);

    } else if (currentExportTarget === "mbox") {
      const outPath = document.getElementById("dest-mbox-path").value.trim();
      if (!outPath) {
        await showCustomAlert("Caminho Obrigatório", "Informe o caminho do arquivo .mbox de saída.", "⚠️");
        return;
      }

      const res = await fetch("/api/export/mbox", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ profile_id: profileId, folder: folder, output_path: outPath })
      });
      if (!res.ok) throw new Error((await res.json()).detail);
      openProgressModal("Exportação MBOX em Andamento", "export", null);
    }
  } catch (err) {
    await showCustomAlert("Falha na Operação", "Falha ao iniciar exportação: " + err.message, "❌");
  }
}

// 8. Ferramentas e Auditoria
async function runIntegrityAudit() {
  const resultBox = document.getElementById("audit-result-box");
  resultBox.style.display = "block";
  document.getElementById("audit-checked").textContent = "Verificando...";

  try {
    const res = await fetch("/api/verify", { method: "POST" });
    const report = await res.json();

    document.getElementById("audit-checked").textContent = report.total_checked;
    document.getElementById("audit-valid").textContent = report.valid_count;
    document.getElementById("audit-missing").textContent = report.missing_count;
    document.getElementById("audit-corrupt").textContent = report.corrupt_count;
  } catch (err) {
    await showCustomAlert("Erro na Auditoria", "Erro ao executar auditoria: " + err.message, "❌");
  }
}

async function loadFullLogs() {
  try {
    const res = await fetch("/api/logs?limit=50");
    const logs = await res.json();
    const tbody = document.getElementById("full-logs-tbody");

    if (!logs || logs.length === 0) {
      tbody.innerHTML = '<tr><td colspan="6" style="text-align: center; color: var(--text-muted); padding: 20px;">Nenhum registro encontrado.</td></tr>';
      return;
    }

    tbody.innerHTML = logs.map(l => `
      <tr>
        <td>${formatDate(l.started_at)}</td>
        <td><strong>${l.action.toUpperCase()}</strong></td>
        <td title="${escapeHtml(l.profile_username || l.profile_id)}">${escapeHtml(l.profile_name || l.profile_id)}</td>
        <td><span class="status-dot" style="background-color: ${getStatusColor(l.status)}"></span> ${l.status}</td>
        <td>${l.messages_count}</td>
        <td>${escapeHtml(l.details || '')}</td>
      </tr>
    `).join("");
  } catch (err) {
    console.error(err);
  }
}

function escapeHtml(str) {
  if (!str) return "";
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}
