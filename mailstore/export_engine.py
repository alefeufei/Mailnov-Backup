"""Mecanismos de exportação para servidores IMAP, diretórios EML e arquivos MBOX."""

import email
import email.policy
import mailbox
import os
import re
import shutil
import time
from datetime import datetime
from pathlib import Path
from typing import Callable, List, Optional
from mailstore.config import STORAGE_DIR
from mailstore.database import Database
from mailstore.imap_engine import (
    ImapClient,
    decode_imap_utf7,
    encode_imap_utf7,
)
from mailstore.models import (
    ExportEmlRequest,
    ExportImapRequest,
    ExportMboxRequest,
    ImapConfig,
)
from mailstore.parser import parse_email_date


class ExportProgress:
    def __init__(self, action: str, total_messages: int, destination: str):
        self.action = action
        self.destination = destination
        self.total_messages = total_messages
        self.current_index = 0
        self.success_count = 0
        self.error_count = 0
        self.status = "running"
        self.percent = 0.0
        self.current_folder = ""
        self.current_subject = ""
        self.log_lines: List[str] = []
        self.is_cancelled = False
        self.start_time = time.time()

    def add_log(self, text: str):
        ts = datetime.now().strftime("%H:%M:%S")
        self.log_lines.append(f"[{ts}] {text}")
        if len(self.log_lines) > 200:
            self.log_lines.pop(0)

    def to_dict(self) -> dict:
        return {
            "action": self.action,
            "destination": self.destination,
            "total_messages": self.total_messages,
            "current_index": self.current_index,
            "success_count": self.success_count,
            "error_count": self.error_count,
            "status": self.status,
            "percent": round(self.percent, 1),
            "current_folder": self.current_folder,
            "current_subject": self.current_subject,
            "logs": self.log_lines[-20:],
        }


class ExportEngine:
    def __init__(self, db: Database):
        self.db = db

    def export_to_imap(
        self,
        req: ExportImapRequest,
        progress: Optional[ExportProgress] = None,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> bool:
        """Migra / restaura e-mails para outro servidor IMAP preservando pastas, flags e datas."""
        target_cfg = ImapConfig(
            host=req.target_host,
            port=req.target_port,
            ssl_mode=req.target_ssl_mode,
            username=req.target_username,
            password=req.target_password
        )

        # Busca mensagens no banco local
        with self.db.get_connection() as conn:
            where_clauses = []
            params = []
            if req.profile_id:
                where_clauses.append("account_id = ?")
                params.append(req.profile_id)
            if req.folder:
                where_clauses.append("folder = ?")
                params.append(req.folder)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
            sql = f"""
                SELECT id, account_id, folder, uid, subject, date_sent, date_received,
                       flags, eml_relative_path
                FROM messages
                {where_sql}
                ORDER BY folder ASC, date_sent ASC
            """
            cur = conn.execute(sql, tuple(params))
            messages = cur.fetchall()

        total = len(messages)
        dest_desc = f"{req.target_username}@{req.target_host}"
        if not progress:
            progress = ExportProgress("export_imap", total, dest_desc)
        else:
            progress.total_messages = total

        if total == 0:
            progress.status = "success"
            progress.add_log("Nenhuma mensagem selecionada para exportação.")
            if on_progress:
                on_progress(progress.to_dict())
            return True

        client = ImapClient(target_cfg)
        try:
            progress.add_log(f"Conectando ao servidor IMAP de destino: {req.target_host}:{req.target_port}...")
            client.connect()
            progress.add_log("Autenticação com o servidor de destino bem-sucedida.")

            # Descobre pastas existentes no destino
            dest_folders_list = client.list_folders()
            existing_dest_folders = {f["name"].lower(): f["raw_name"] for f in dest_folders_list}

            created_folders_cache = set()

            for idx, msg in enumerate(messages, 1):
                if progress.is_cancelled:
                    progress.status = "cancelled"
                    progress.add_log("Migração IMAP cancelada pelo usuário.")
                    break

                progress.current_index = idx
                progress.percent = (idx / total) * 100
                progress.current_folder = msg["folder"]
                progress.current_subject = msg["subject"] or "(Sem assunto)"

                # Determina nome da pasta no destino
                folder_dest = msg["folder"]
                if req.target_folder_prefix:
                    prefix = req.target_folder_prefix.strip("/")
                    folder_dest = f"{prefix}/{folder_dest}" if folder_dest.lower() != "inbox" else f"{prefix}/INBOX"

                # Cria pasta no destino se necessário
                if folder_dest.lower() not in existing_dest_folders and folder_dest not in created_folders_cache:
                    try:
                        encoded_target = encode_imap_utf7(folder_dest)
                        typ, create_resp = client.conn.create(f'"{encoded_target}"')
                        if typ == "OK":
                            existing_dest_folders[folder_dest.lower()] = encoded_target
                            created_folders_cache.add(folder_dest)
                            progress.add_log(f"Pasta criada no destino: {folder_dest}")
                    except Exception as cr_err:
                        # Pasta pode já existir ou servidor retornar aviso
                        created_folders_cache.add(folder_dest)

                # Carrega bytes do arquivo .eml
                eml_abs = STORAGE_DIR / msg["eml_relative_path"]
                if not eml_abs.exists():
                    progress.add_log(f"Arquivo EML ausente: {msg['eml_relative_path']}")
                    progress.error_count += 1
                    continue

                raw_bytes = eml_abs.read_bytes()

                # Prepara Flags
                flags_to_send = None
                if req.preserve_flags and msg["flags"]:
                    # Converte formato (\\Seen \\Flagged) em tupla
                    fl_list = [f for f in msg["flags"].split() if f.startswith("\\")]
                    if fl_list:
                        flags_to_send = " ".join(fl_list)

                # Prepara InternalDate
                internal_date_val = None
                if req.preserve_internal_date:
                    date_ref = msg["date_received"] or msg["date_sent"]
                    if date_ref:
                        try:
                            dt = datetime.fromisoformat(date_ref)
                            import imaplib
                            internal_date_val = imaplib.Time2Internaldate(dt.timestamp())
                        except Exception:
                            pass

                # APPEND no servidor
                try:
                    folder_to_append = existing_dest_folders.get(folder_dest.lower()) or encode_imap_utf7(folder_dest)
                    # Formato seguro de aspas para pasta no APPEND
                    folder_arg = f'"{folder_to_append}"' if not folder_to_append.startswith('"') else folder_to_append

                    typ, resp = client.conn.append(
                        folder_arg,
                        flags_to_send,
                        internal_date_val,
                        raw_bytes
                    )
                    if typ == "OK":
                        progress.success_count += 1
                    else:
                        progress.add_log(f"Erro APPEND na mensagem {msg['id']}: {resp}")
                        progress.error_count += 1

                except Exception as app_err:
                    progress.add_log(f"Falha ao enviar mensagem {msg['id']}: {app_err}")
                    progress.error_count += 1

                if idx % 10 == 0 or idx == total:
                    if on_progress:
                        on_progress(progress.to_dict())

            progress.status = "success" if progress.error_count == 0 else "partial"
            progress.add_log(
                f"Migração IMAP finalizada. Enviadas com sucesso: {progress.success_count}, Falhas: {progress.error_count}."
            )

        except Exception as conn_err:
            progress.status = "error"
            progress.add_log(f"Falha de conexão com servidor destino: {conn_err}")
            return False

        finally:
            client.disconnect()

        if on_progress:
            on_progress(progress.to_dict())

        return progress.status == "success"

    def export_to_eml_directory(
        self,
        req: ExportEmlRequest,
        progress: Optional[ExportProgress] = None,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> bool:
        """Exporta hierarquia de pastas completa com arquivos .eml individuais."""
        out_root = Path(req.output_dir).resolve()
        out_root.mkdir(parents=True, exist_ok=True)

        with self.db.get_connection() as conn:
            where_clauses = []
            params = []
            if req.profile_id:
                where_clauses.append("account_id = ?")
                params.append(req.profile_id)
            if req.folder:
                where_clauses.append("folder = ?")
                params.append(req.folder)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
            sql = f"SELECT id, folder, subject, date_sent, eml_relative_path FROM messages{where_sql} ORDER BY folder ASC, id ASC"
            cur = conn.execute(sql, tuple(params))
            messages = cur.fetchall()

        total = len(messages)
        if not progress:
            progress = ExportProgress("export_eml", total, str(out_root))
        else:
            progress.total_messages = total

        for idx, msg in enumerate(messages, 1):
            if progress.is_cancelled:
                progress.status = "cancelled"
                break

            progress.current_index = idx
            progress.percent = (idx / max(1, total)) * 100
            progress.current_folder = msg["folder"]
            progress.current_subject = msg["subject"] or "(Sem assunto)"

            eml_abs = STORAGE_DIR / msg["eml_relative_path"]
            if not eml_abs.exists():
                progress.error_count += 1
                continue

            # Sanitização de caminhos
            folder_clean = re.sub(r'[\/:*?"<>|]', '_', msg["folder"]).strip(". ") or "Root"
            target_folder = out_root / folder_clean
            target_folder.mkdir(parents=True, exist_ok=True)

            date_prefix = ""
            if msg["date_sent"]:
                date_prefix = msg["date_sent"][:10] + "_"

            subj_clean = re.sub(r'[\/:*?"<>|]', '_', msg["subject"] or "sem_assunto")[:60].strip()
            target_file = target_folder / f"{date_prefix}{msg['id']}_{subj_clean}.eml"

            try:
                shutil.copy2(eml_abs, target_file)
                progress.success_count += 1
            except Exception as copy_err:
                progress.add_log(f"Erro copiando EML {msg['id']}: {copy_err}")
                progress.error_count += 1

            if idx % 20 == 0 or idx == total:
                if on_progress:
                    on_progress(progress.to_dict())

        progress.status = "success" if progress.error_count == 0 else "partial"
        progress.add_log(f"Exportação EML concluída em {out_root}: {progress.success_count} arquivos gerados.")
        if on_progress:
            on_progress(progress.to_dict())
        return True

    def export_to_mbox(
        self,
        req: ExportMboxRequest,
        progress: Optional[ExportProgress] = None,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> bool:
        """Exporta mensagens para arquivo MBOX padrão compatível com Thunderbird."""
        out_file = Path(req.output_path).resolve()
        out_file.parent.mkdir(parents=True, exist_ok=True)

        if out_file.exists():
            try:
                out_file.unlink()
            except Exception:
                pass

        with self.db.get_connection() as conn:
            where_clauses = []
            params = []
            if req.profile_id:
                where_clauses.append("account_id = ?")
                params.append(req.profile_id)
            if req.folder:
                where_clauses.append("folder = ?")
                params.append(req.folder)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
            sql = f"SELECT id, folder, subject, eml_relative_path FROM messages{where_sql} ORDER BY folder ASC, id ASC"
            cur = conn.execute(sql, tuple(params))
            messages = cur.fetchall()

        total = len(messages)
        if not progress:
            progress = ExportProgress("export_mbox", total, str(out_file))
        else:
            progress.total_messages = total

        box = mailbox.mbox(str(out_file))
        box.lock()

        try:
            for idx, msg in enumerate(messages, 1):
                if progress.is_cancelled:
                    progress.status = "cancelled"
                    break

                progress.current_index = idx
                progress.percent = (idx / max(1, total)) * 100
                progress.current_subject = msg["subject"] or ""

                eml_abs = STORAGE_DIR / msg["eml_relative_path"]
                if not eml_abs.exists():
                    progress.error_count += 1
                    continue

                try:
                    with open(eml_abs, "rb") as f:
                        box.add(f)
                    progress.success_count += 1
                except Exception as mbox_err:
                    progress.add_log(f"Falha adicionando ao MBOX: {mbox_err}")
                    progress.error_count += 1

                if idx % 20 == 0 or idx == total:
                    if on_progress:
                        on_progress(progress.to_dict())

            box.flush()
        finally:
            box.unlock()
            box.close()

        progress.status = "success" if progress.error_count == 0 else "partial"
        progress.add_log(f"Exportação MBOX concluída em {out_file}: {progress.success_count} mensagens.")
        if on_progress:
            on_progress(progress.to_dict())
        return True
