"""Servidor REST API e servidor web embutido do MailStore Backup."""

import asyncio
from mailstore import __version__
import os
import subprocess
import sys
import threading
from pathlib import Path
from typing import Dict, Optional
from fastapi import FastAPI, HTTPException, Query, Response, status
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse, HTMLResponse
from fastapi.staticfiles import StaticFiles

from mailstore.config import BASE_DIR, STORAGE_DIR, detect_provider
from mailstore.database import Database
from mailstore.export_engine import ExportEngine, ExportProgress
from mailstore.imap_engine import (
    BackupProgress,
    BackupRunner,
    ImapClient,
    test_imap_connection,
)
from mailstore.models import (
    BackupProfile,
    ExportEmlRequest,
    ExportImapRequest,
    ExportMboxRequest,
    ExportPstRequest,
    ImapConfig,
    ProfileCreateRequest,
    ProfileUpdateRequest,
    SearchQuery,
    TestConnectionRequest,
)
from mailstore.pst_engine import PstExportProgress, PstExporter, is_outlook_com_available
from mailstore.verifier import ArchiveVerifier

# Instância FastAPI
app = FastAPI(
    title="Mailnov Backup",
    description="Sistema profissional de arquivamento, backup e migração de e-mails IMAP",
    version=__version__
)

# CORS liberado para interface local
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

db = Database()
imap_runner = BackupRunner(db)
export_engine = ExportEngine(db)
pst_exporter = PstExporter(db)
verifier = ArchiveVerifier(db)

# Gerenciador de jobs em background
active_backup_jobs: Dict[str, BackupProgress] = {}
active_export_job: Optional[ExportProgress] = None
active_pst_job: Optional[PstExportProgress] = None
job_lock = threading.Lock()


# --- Rotas de Status e Diagnóstico ---

@app.get("/api/status")
def get_system_status():
    stats = db.get_system_stats()
    pst_ok, pst_msg = is_outlook_com_available()
    return {
        "status": "online",
        "version": __version__,
        "data_dir": str(BASE_DIR),
        "db_size_bytes": os.path.getsize(db.db_path) if db.db_path.exists() else 0,
        "pst_support": {
            "available": pst_ok,
            "message": pst_msg,
        },
        **stats
    }


@app.get("/api/detect-provider")
def api_detect_provider(email: str = Query(...)):
    detected = detect_provider(email)
    return {"detected": detected}


@app.post("/api/test-connection")
def api_test_connection(req: TestConnectionRequest):
    cfg = ImapConfig(
        host=req.host,
        port=req.port,
        ssl_mode=req.ssl_mode,
        username=req.username,
        password=req.password,
        timeout=getattr(req, "timeout", 30)
    )
    res = test_imap_connection(cfg)
    return res


# --- Rotas de Perfis ---

@app.get("/api/profiles")
def list_profiles():
    return db.list_profiles()


@app.post("/api/profiles")
def create_profile(req: ProfileCreateRequest):
    import uuid
    new_id = str(uuid.uuid4())[:8]
    profile = BackupProfile(
        id=new_id,
        name=req.name,
        imap=req.imap,
        selected_folders=req.selected_folders,
        exclude_folders=req.exclude_folders,
        since_date=req.since_date
    )
    db.save_profile(profile)
    return profile


@app.get("/api/profiles/{profile_id}")
def get_profile(profile_id: str):
    p = db.get_profile(profile_id)
    if not p:
        raise HTTPException(status_code=404, detail="Perfil não encontrado.")
    return p


@app.put("/api/profiles/{profile_id}")
def update_profile(profile_id: str, req: ProfileUpdateRequest):
    p = db.get_profile(profile_id)
    if not p:
        raise HTTPException(status_code=404, detail="Perfil não encontrado.")
    if req.name is not None:
        p.name = req.name
    if req.imap is not None:
        p.imap = req.imap
    if req.selected_folders is not None:
        p.selected_folders = req.selected_folders
    if req.exclude_folders is not None:
        p.exclude_folders = req.exclude_folders
    if req.since_date is not None:
        p.since_date = req.since_date
    db.save_profile(p)
    return p


@app.delete("/api/profiles/{profile_id}")
def delete_profile(profile_id: str):
    p = db.get_profile(profile_id)
    if not p:
        raise HTTPException(status_code=404, detail="Perfil não encontrado.")
    db.delete_profile(profile_id)
    return {"success": True, "message": f"Perfil '{p.name}' e seus índices foram removidos com sucesso."}


# --- Execução de Backup ---

@app.post("/api/profiles/{profile_id}/backup")
def start_backup(profile_id: str):
    p = db.get_profile(profile_id)
    if not p:
        raise HTTPException(status_code=404, detail="Perfil não encontrado.")

    with job_lock:
        if profile_id in active_backup_jobs and active_backup_jobs[profile_id].status == "running":
            raise HTTPException(status_code=400, detail="Já existe um backup em andamento para este perfil.")
        
        progress = BackupProgress(profile_id)
        active_backup_jobs[profile_id] = progress

    def worker():
        try:
            imap_runner.run_backup(p, progress)
        except Exception as e:
            progress.status = "error"
            progress.add_log(f"Erro inesperado: {e}")

    thread = threading.Thread(target=worker, daemon=True)
    thread.start()

    return {"success": True, "message": "Backup iniciado com sucesso.", "job": progress.to_dict()}


@app.get("/api/profiles/{profile_id}/progress")
def get_backup_progress(profile_id: str):
    with job_lock:
        job = active_backup_jobs.get(profile_id)
        if not job:
            p = db.get_profile(profile_id)
            return {
                "profile_id": profile_id,
                "status": p.last_status if p else "idle",
                "percent": 0.0,
                "current_msg": 0,
                "total_msgs": 0,
                "logs": [],
            }
        return job.to_dict()


@app.post("/api/profiles/{profile_id}/cancel")
def cancel_backup(profile_id: str):
    with job_lock:
        job = active_backup_jobs.get(profile_id)
        if not job or job.status != "running":
            raise HTTPException(status_code=400, detail="Nenhum backup em andamento para este perfil.")
        job.is_cancelled = True
        return {"success": True, "message": "Sinal de cancelamento enviado."}


# --- Pastas e Mensagens ---
 
@app.get("/api/folders")
def get_folders(account_id: Optional[str] = None):
    return db.get_folders(account_id)


@app.get("/api/browse/tree")
def get_browse_tree():
    return db.get_browse_tree()


@app.get("/api/messages")
def list_messages(
    account_id: Optional[str] = None,
    folder: Optional[str] = None,
    limit: int = 50,
    offset: int = 0
):
    items, total = db.list_messages(account_id, folder, limit, offset)
    return {
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }


@app.post("/api/search")
def search_messages(q: SearchQuery):
    items, total = db.search_messages(q)
    return {
        "items": items,
        "total": total,
        "limit": q.limit,
        "offset": q.offset,
    }


@app.get("/api/messages/{message_id}")
def get_message_detail(message_id: int):
    detail = db.get_message_detail(message_id)
    if not detail:
        raise HTTPException(status_code=404, detail="Mensagem não encontrada.")
    
    # Se o corpo HTML existir no EML bruto, carrega sob demanda
    eml_file = STORAGE_DIR / detail.eml_relative_path
    if eml_file.exists():
        try:
            from mailstore.parser import ParsedEmail
            parsed = ParsedEmail(eml_file.read_bytes(), detail.account_id, detail.folder, detail.uid)
            detail.body_html = parsed.body_html
            if not detail.body_text:
                detail.body_text = parsed.body_text
        except Exception:
            pass

    return detail


@app.get("/api/messages/{message_id}/eml")
def download_eml(message_id: int):
    detail = db.get_message_detail(message_id)
    if not detail:
        raise HTTPException(status_code=404, detail="Mensagem não encontrada.")
    
    eml_path = STORAGE_DIR / detail.eml_relative_path
    if not eml_path.exists():
        raise HTTPException(status_code=404, detail="Arquivo EML físico não encontrado no repositório.")
    
    filename = f"{detail.uid}_{detail.subject[:40].strip() or 'email'}.eml"
    return FileResponse(
        path=str(eml_path),
        filename=filename,
        media_type="message/rfc822"
    )


@app.get("/api/messages/{message_id}/attachments/{part_index}")
def download_attachment(message_id: int, part_index: int):
    detail = db.get_message_detail(message_id)
    if not detail:
        raise HTTPException(status_code=404, detail="Mensagem não encontrada.")
    
    eml_path = STORAGE_DIR / detail.eml_relative_path
    if not eml_path.exists():
        raise HTTPException(status_code=404, detail="Arquivo EML não encontrado.")
    
    from mailstore.parser import ParsedEmail
    parsed = ParsedEmail(eml_path.read_bytes(), detail.account_id, detail.folder, detail.uid)

    target_att = None
    for att in parsed.attachments:
        if att["part_index"] == part_index:
            target_att = att
            break
    
    if not target_att or not target_att.get("payload"):
        raise HTTPException(status_code=404, detail="Anexo não localizado.")

    return Response(
        content=target_att["payload"],
        media_type=target_att.get("content_type", "application/octet-stream"),
        headers={
            "Content-Disposition": f'attachment; filename="{target_att["filename"]}"'
        }
    )


# --- Rotas de Exportação ---

@app.post("/api/export/imap")
def export_imap(req: ExportImapRequest):
    global active_export_job
    with job_lock:
        if active_export_job and active_export_job.status == "running":
            raise HTTPException(status_code=400, detail="Já existe uma exportação em andamento.")
        progress = ExportProgress("export_imap", 0, f"{req.target_username}@{req.target_host}")
        active_export_job = progress

    def worker():
        try:
            export_engine.export_to_imap(req, progress)
        except Exception as e:
            progress.status = "error"
            progress.add_log(f"Erro na migração IMAP: {e}")

    threading.Thread(target=worker, daemon=True).start()
    return {"success": True, "message": "Migração IMAP iniciada.", "job": progress.to_dict()}


@app.post("/api/export/pst")
def export_pst(req: ExportPstRequest):
    global active_pst_job
    pst_ok, pst_msg = is_outlook_com_available()
    if not pst_ok:
        raise HTTPException(status_code=400, detail=f"Criação de PST requer Microsoft Outlook: {pst_msg}")

    with job_lock:
        if active_pst_job and active_pst_job.status == "running":
            raise HTTPException(status_code=400, detail="Já existe uma exportação PST em andamento.")
        progress = PstExportProgress(0, req.output_path)
        active_pst_job = progress

    def worker():
        try:
            pst_exporter.export_to_pst(
                output_pst_path=req.output_path,
                account_id=req.profile_id,
                folder_filter=req.folder,
                progress=progress
            )
        except Exception as e:
            progress.status = "error"
            progress.add_log(f"Erro na exportação PST: {e}")

    threading.Thread(target=worker, daemon=True).start()
    return {"success": True, "message": "Exportação PST iniciada.", "job": progress.to_dict()}


@app.post("/api/export/eml")
def export_eml(req: ExportEmlRequest):
    global active_export_job
    with job_lock:
        if active_export_job and active_export_job.status == "running":
            raise HTTPException(status_code=400, detail="Já existe uma exportação em andamento.")
        progress = ExportProgress("export_eml", 0, req.output_dir)
        active_export_job = progress

    def worker():
        try:
            export_engine.export_to_eml_directory(req, progress)
        except Exception as e:
            progress.status = "error"
            progress.add_log(f"Erro na exportação EML: {e}")

    threading.Thread(target=worker, daemon=True).start()
    return {"success": True, "message": "Exportação EML iniciada.", "job": progress.to_dict()}


@app.post("/api/export/mbox")
def export_mbox(req: ExportMboxRequest):
    global active_export_job
    with job_lock:
        if active_export_job and active_export_job.status == "running":
            raise HTTPException(status_code=400, detail="Já existe uma exportação em andamento.")
        progress = ExportProgress("export_mbox", 0, req.output_path)
        active_export_job = progress

    def worker():
        try:
            export_engine.export_to_mbox(req, progress)
        except Exception as e:
            progress.status = "error"
            progress.add_log(f"Erro na exportação MBOX: {e}")

    threading.Thread(target=worker, daemon=True).start()
    return {"success": True, "message": "Exportação MBOX iniciada.", "job": progress.to_dict()}


@app.get("/api/jobs/export/progress")
def get_export_progress():
    global active_export_job, active_pst_job
    with job_lock:
        if active_pst_job and active_pst_job.status == "running":
            return active_pst_job.to_dict()
        if active_export_job:
            return active_export_job.to_dict()
        if active_pst_job:
            return active_pst_job.to_dict()
        return {"status": "idle", "percent": 0.0, "logs": []}


@app.post("/api/jobs/export/cancel")
def cancel_export_job():
    global active_export_job, active_pst_job
    with job_lock:
        cancelled = False
        if active_export_job and active_export_job.status == "running":
            active_export_job.is_cancelled = True
            cancelled = True
        if active_pst_job and active_pst_job.status == "running":
            active_pst_job.is_cancelled = True
            cancelled = True
        if not cancelled:
            raise HTTPException(status_code=400, detail="Nenhuma exportação em andamento.")
        return {"success": True, "message": "Cancelamento solicitado."}


@app.get("/api/dialog/save-file")
def dialog_save_file(
    default_name: str = "backup_emails.pst",
    file_type: str = "pst",
    title: str = "Salvar Arquivo PST do Outlook"
):
    """Abre o diálogo nativo do Windows para o usuário escolher o local e nome de arquivo."""
    try:
        clean_ext = file_type.lstrip(".").strip()
        script = f"""
import tkinter as tk
from tkinter import filedialog
root = tk.Tk()
root.withdraw()
root.attributes("-topmost", True)
path = filedialog.asksaveasfilename(
    title="{title}",
    initialfile="{default_name}",
    defaultextension=".{clean_ext}",
    filetypes=[("Arquivo {clean_ext.upper()}", "*.{clean_ext}"), ("Todos os arquivos", "*.*")]
)
root.destroy()
if path:
    import os
    print(os.path.normpath(path))
"""
        res = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=120)
        selected_path = res.stdout.strip()
        return {"path": selected_path}
    except Exception as e:
        return {"path": "", "error": str(e)}


@app.get("/api/dialog/select-folder")
def dialog_select_folder(title: str = "Selecionar Pasta"):
    """Abre o diálogo nativo do Windows para o usuário escolher uma pasta no disco."""
    try:
        script = f"""
import tkinter as tk
from tkinter import filedialog
root = tk.Tk()
root.withdraw()
root.attributes("-topmost", True)
path = filedialog.askdirectory(title="{title}")
root.destroy()
if path:
    import os
    print(os.path.normpath(path))
"""
        res = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=120)
        selected_path = res.stdout.strip()
        return {"path": selected_path}
    except Exception as e:
        return {"path": "", "error": str(e)}


@app.get("/api/fs/browse")
def browse_filesystem(path: Optional[str] = None):
    """Navega pelo sistema de arquivos local para escolha visual de pastas e arquivos."""
    import string
    home_dir = Path.home()
    if not path or not path.strip():
        target = home_dir / "Desktop"
        if not target.exists():
            target = home_dir
    else:
        target = Path(path).resolve()
        if not target.exists() or not target.is_dir():
            target = target.parent if (target.parent.exists() and target.parent.is_dir()) else home_dir

    shortcuts = [
        {"name": "Área de Trabalho", "path": str(home_dir / "Desktop"), "icon": "🖥️"},
        {"name": "Documentos", "path": str(home_dir / "Documents"), "icon": "📁"},
        {"name": "Downloads", "path": str(home_dir / "Downloads"), "icon": "📥"},
        {"name": "Pasta do Projeto", "path": str(BASE_DIR), "icon": "📦"},
    ]
    shortcuts = [s for s in shortcuts if Path(s["path"]).exists()]

    drives = []
    if os.name == "nt":
        for letter in string.ascii_uppercase:
            drive_path = f"{letter}:\\"
            if os.path.exists(drive_path):
                drives.append(drive_path)

    directories = []
    try:
        with os.scandir(target) as it:
            for entry in it:
                try:
                    if entry.is_dir(follow_symlinks=False) and not entry.name.startswith("."):
                        directories.append({
                            "name": entry.name,
                            "path": str(Path(entry.path).resolve())
                        })
                except (PermissionError, OSError):
                    continue
        directories.sort(key=lambda d: d["name"].lower())
    except (PermissionError, OSError):
        pass

    parent_path = str(target.parent) if target.parent != target else ""

    return {
        "current_path": str(target),
        "parent_path": parent_path,
        "drives": drives,
        "shortcuts": shortcuts,
        "directories": directories
    }


@app.post("/api/fs/create-folder")
def create_filesystem_folder(req: dict):
    parent = req.get("parent_path", "").strip()
    name = req.get("folder_name", "").strip()
    if not parent or not name:
        raise HTTPException(status_code=400, detail="Caminho e nome da pasta são obrigatórios.")
    new_dir = Path(parent) / name
    try:
        new_dir.mkdir(parents=True, exist_ok=True)
        return {"success": True, "created_path": str(new_dir)}
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Erro ao criar pasta: {e}")


# --- Verificação de Integridade e Logs ---

@app.post("/api/verify")
def run_verification(account_id: Optional[str] = None):
    rep = verifier.verify_archive(account_id)
    return rep.to_dict()


@app.get("/api/logs")
def get_logs(limit: int = 50):
    return db.get_logs(limit)


# --- Servir Interface Web Estática (SPA) ---

WEB_DIR = Path(__file__).parent / "web"
if WEB_DIR.exists():
    app.mount("/static", StaticFiles(directory=str(WEB_DIR)), name="static")

    @app.get("/")
    def serve_index():
        index_file = WEB_DIR / "index.html"
        if index_file.exists():
            return HTMLResponse(content=index_file.read_text(encoding="utf-8"))
        return HTMLResponse("<h1>Mailnov Backup</h1><p>Frontend não encontrado.</p>")

    @app.get("/favicon.ico", include_in_schema=False)
    def serve_favicon():
        ico_file = WEB_DIR / "favicon.ico"
        if ico_file.exists():
            return FileResponse(ico_file, media_type="image/x-icon")
        return Response(status_code=404)
