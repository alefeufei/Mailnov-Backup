// MailStore Backup Studio - Frontend JavaScript

let currentActiveTab = "tab-dashboard";
let currentPollingJob = null;
let currentExportTarget = "imap";
let activeViewerEmailId = null;

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
        <td>${l.profile_id}</td>
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
    alert("Por favor, preencha o e-mail, senha e servidor IMAP.");
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
    alert("Erro ao salvar perfil: " + err.message);
  }
}

async function editProfile(id) {
  const res = await fetch(`/api/profiles/${id}`);
  const profile = await res.json();
  openProfileModal(profile);
}

async function deleteProfile(id) {
  if (!confirm("Deseja realmente remover este perfil? O índice de busca desta conta será excluído.")) return;
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
      alert("Erro ao iniciar backup: " + err.detail);
      return;
    }
    openProgressModal("Backup IMAP em Andamento", "backup", profileId);
  } catch (err) {
    alert("Erro de comunicação: " + err.message);
  }
}

function openProgressModal(title, jobType, targetId) {
  document.getElementById("progress-modal-title").textContent = title;
  document.getElementById("progress-bar-fill").style.width = "0%";
  document.getElementById("progress-percent-label").textContent = "0%";
  document.getElementById("progress-log-terminal").innerHTML = "";
  document.getElementById("btn-cancel-job").style.display = "inline-flex";
  document.getElementById("btn-close-progress").style.display = "none";
  document.getElementById("modal-progress").classList.add("active");

  if (currentPollingJob) clearInterval(currentPollingJob);

  currentPollingJob = setInterval(async () => {
    try {
      let url = jobType === "backup" ? `/api/profiles/${targetId}/progress` : "/api/jobs/export/progress";
      const res = await fetch(url);
      const data = await res.json();

      document.getElementById("progress-bar-fill").style.width = `${data.percent || 0}%`;
      document.getElementById("progress-percent-label").textContent = `${data.percent || 0}%`;
      
      if (jobType === "backup") {
        document.getElementById("progress-details-current").textContent = `Pasta: ${data.current_folder || '-'} (${data.current_msg || 0}/${data.total_msgs || 0})`;
        document.getElementById("progress-speed").textContent = `${data.speed_mps || 0} msgs/s`;
      } else {
        document.getElementById("progress-details-current").textContent = `Progresso: ${data.current_index || 0}/${data.total_messages || 0}`;
        document.getElementById("progress-speed").textContent = data.current_subject ? data.current_subject.substring(0, 30) : "";
      }

      // Logs
      if (data.logs) {
        document.getElementById("progress-log-terminal").innerHTML = data.logs.join("<br>");
        const term = document.getElementById("progress-log-terminal");
        term.scrollTop = term.scrollHeight;
      }

      // Finalização
      if (data.status !== "running") {
        clearInterval(currentPollingJob);
        currentPollingJob = null;
        document.getElementById("btn-cancel-job").style.display = "none";
        document.getElementById("btn-close-progress").style.display = "inline-flex";
        document.getElementById("progress-status-desc").textContent = `Operação concluída (${data.status}).`;

        loadDashboardData();
        loadProfiles();
        loadFoldersTree();
      }
    } catch (err) {
      console.error(err);
    }
  }, 750);
}

function closeProgressModal() {
  if (currentPollingJob) clearInterval(currentPollingJob);
  currentPollingJob = null;
  document.getElementById("modal-progress").classList.remove("active");
}

async function cancelCurrentJob() {
  await fetch("/api/jobs/export/cancel", { method: "POST" });
  document.getElementById("progress-status-desc").textContent = "Cancelamento solicitado...";
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
            <span class="profile-toggle-btn" id="toggle-btn-${account.account_id}" onclick="event.stopPropagation(); toggleProfileFolders('${escapeHtml(account.account_id)}')">▼</span>
            <span style="font-size: 15px;">📬</span>
            <div class="profile-info">
              <div class="profile-name" title="${escapeHtml(account.account_name)}">${escapeHtml(account.account_name)}</div>
              <div class="profile-sub" title="${escapeHtml(account.username)}">${escapeHtml(account.username)}</div>
            </div>
            <span class="badge-count" title="Total na conta: ${totalMsgs}">${totalMsgs}</span>
          </div>

          <div class="profile-folders-container" id="folders-container-${account.account_id}">
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
  if (container) container.classList.toggle("collapsed");
  if (btn) btn.classList.toggle("collapsed");
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
    alert("Erro ao carregar detalhes da mensagem: " + err.message);
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

async function loadExportAccounts() {
  try {
    const res = await fetch("/api/profiles");
    const profiles = await res.json();
    const sel = document.getElementById("export-source-profile");
    sel.innerHTML = '<option value="">Todas as contas arquivadas</option>' +
      profiles.map(p => `<option value="${p.id}">${escapeHtml(p.name)} (${p.imap.username})</option>`).join("");
    loadFoldersForExport();
  } catch (err) {
    console.error(err);
  }
}

async function loadFoldersForExport() {
  const accountId = document.getElementById("export-source-profile").value;
  const sel = document.getElementById("export-source-folder");
  if (!accountId) {
    sel.innerHTML = '<option value="">Todas as pastas</option>';
    return;
  }
  const res = await fetch(`/api/folders?account_id=${encodeURIComponent(accountId)}`);
  const folders = await res.json();
  sel.innerHTML = '<option value="">Todas as pastas</option>' +
    folders.map(f => `<option value="${escapeHtml(f.folder_name)}">${escapeHtml(f.folder_name)} (${f.message_count})</option>`).join("");
}

async function startExport() {
  const profileId = document.getElementById("export-source-profile").value || null;
  const folder = document.getElementById("export-source-folder").value || null;

  try {
    if (currentExportTarget === "imap") {
      const host = document.getElementById("dest-imap-host").value.trim();
      const port = parseInt(document.getElementById("dest-imap-port").value) || 993;
      const ssl = document.getElementById("dest-imap-ssl").value;
      const user = document.getElementById("dest-imap-user").value.trim();
      const pass = document.getElementById("dest-imap-pass").value;
      const prefix = document.getElementById("dest-imap-prefix").value.trim();

      if (!host || !user || !pass) {
        alert("Preencha o servidor, usuário e senha do IMAP destino.");
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
        alert("Informe o caminho do arquivo .pst de saída.");
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
        alert("Informe o diretório de destino.");
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
        alert("Informe o caminho do arquivo .mbox de saída.");
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
    alert("Falha ao iniciar exportação: " + err.message);
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
    alert("Erro ao executar auditoria: " + err.message);
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
        <td>${l.profile_id}</td>
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
