"""Mecanismo de exportação nativa para arquivos PST do Microsoft Outlook (.pst)."""

import os
import shutil
import tempfile
from datetime import datetime
from pathlib import Path
from typing import Callable, List, Optional, Tuple
from mailstore.config import STORAGE_DIR
from mailstore.database import Database
from mailstore.parser import ParsedEmail


class PstExportProgress:
    def __init__(self, total_messages: int, output_path: str):
        self.output_path = output_path
        self.total_messages = total_messages
        self.current_index = 0
        self.success_count = 0
        self.error_count = 0
        self.status = "running"
        self.percent = 0.0
        self.current_subject = ""
        self.current_folder = ""
        self.log_lines: List[str] = []
        self.is_cancelled = False

    def add_log(self, text: str):
        ts = datetime.now().strftime("%H:%M:%S")
        self.log_lines.append(f"[{ts}] {text}")
        if len(self.log_lines) > 200:
            self.log_lines.pop(0)

    def to_dict(self) -> dict:
        return {
            "output_path": self.output_path,
            "total_messages": self.total_messages,
            "current_index": self.current_index,
            "success_count": self.success_count,
            "error_count": self.error_count,
            "status": self.status,
            "percent": round(self.percent, 1),
            "current_subject": self.current_subject,
            "current_folder": self.current_folder,
            "logs": self.log_lines[-20:],
        }


def is_outlook_com_available() -> Tuple[bool, str]:
    """Verifica se o Microsoft Outlook e a automação MAPI / COM estão disponíveis no Windows."""
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CLASSES_ROOT, "Outlook.Application"):
            pass
        import win32com.client
        return True, "Microsoft Outlook MAPI disponível."
    except Exception as e:
        return False, f"Outlook MAPI não acessível: {e}"


class PstExporter:
    def __init__(self, db: Database):
        self.db = db

    def export_to_pst(
        self,
        output_pst_path: str,
        account_id: Optional[str] = None,
        folder_filter: Optional[str] = None,
        progress: Optional[PstExportProgress] = None,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> bool:
        """Cria um arquivo PST genuíno do Outlook e insere todas as mensagens e pastas."""
        import win32com.client

        output_path = Path(output_pst_path).resolve()
        output_path.parent.mkdir(parents=True, exist_ok=True)

        # Se o arquivo já existe, remove para recriar limpo
        if output_path.exists():
            try:
                output_path.unlink()
            except Exception as e:
                if progress:
                    progress.add_log(f"Erro ao remover PST existente: {e}")
                raise

        # Consulta mensagens a exportar
        with self.db.get_connection() as conn:
            where_clauses = []
            params = []
            if account_id:
                where_clauses.append("account_id = ?")
                params.append(account_id)
            if folder_filter:
                where_clauses.append("folder = ?")
                params.append(folder_filter)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
            sql = f"SELECT id, account_id, folder, subject, eml_relative_path FROM messages{where_sql} ORDER BY folder ASC, id ASC"
            cur = conn.execute(sql, tuple(params))
            messages_to_export = cur.fetchall()

        total = len(messages_to_export)
        if not progress:
            progress = PstExportProgress(total, str(output_path))
        else:
            progress.total_messages = total

        if total == 0:
            progress.status = "warning"
            progress.add_log("Atenção: Nenhuma mensagem encontrada no arquivo local para exportação PST.")
            if on_progress:
                on_progress(progress.to_dict())
            return False

        import pythoncom
        import win32com.client

        pythoncom.CoInitialize()
        try:
            outlook = win32com.client.Dispatch("Outlook.Application")
            ns = outlook.GetNamespace("MAPI")

            # Cria / conecta a nova store PST
            ns.AddStore(str(output_path))

            target_store = None
            for s in ns.Stores:
                try:
                    if Path(s.FilePath).resolve() == output_path:
                        target_store = s
                        break
                except Exception:
                    continue

            if not target_store:
                raise RuntimeError(f"Não foi possível localizar o store PST criado no Outlook: {output_path}")

            root_folder = target_store.GetRootFolder()
            progress.add_log(f"Repositório PST inicializado: '{root_folder.Name}'")

            # Cache de referências de pastas para não recriar
            folder_cache = {}

            def get_or_create_folder(folder_path: str):
                if folder_path in folder_cache:
                    return folder_cache[folder_path]

                parts = [p.strip() for p in folder_path.replace("\\", "/").split("/") if p.strip()]
                current = root_folder

                for part in parts:
                    found = None
                    for existing in current.Folders:
                        if existing.Name.lower() == part.lower():
                            found = existing
                            break
                    if not found:
                        found = current.Folders.Add(part)
                    current = found

                folder_cache[folder_path] = current
                return current

            temp_dir = Path(tempfile.mkdtemp(prefix="pst_exp_"))

            try:
                for idx, row in enumerate(messages_to_export, 1):
                    if progress.is_cancelled:
                        progress.status = "cancelled"
                        progress.add_log("Exportação PST cancelada.")
                        break

                    progress.current_index = idx
                    progress.percent = (idx / total) * 100
                    progress.current_subject = row["subject"] or "(Sem assunto)"
                    progress.current_folder = row["folder"]

                    eml_abs_path = STORAGE_DIR / row["eml_relative_path"]
                    if not eml_abs_path.exists():
                        progress.add_log(f"Arquivo EML não encontrado: {row['eml_relative_path']}")
                        progress.error_count += 1
                        continue

                    try:
                        raw_bytes = eml_abs_path.read_bytes()
                        parsed = ParsedEmail(raw_bytes, row["account_id"], row["folder"], idx)

                        pst_folder = get_or_create_folder(row["folder"])

                        # 0 = olMailItem
                        mail_item = pst_folder.Items.Add(0)
                        mail_item.Subject = parsed.subject
                        mail_item.To = parsed.recipients
                        if parsed.cc:
                            mail_item.CC = parsed.cc
                        if parsed.bcc:
                            mail_item.BCC = parsed.bcc

                        # Corpo
                        if parsed.body_html:
                            mail_item.HTMLBody = parsed.body_html
                        elif parsed.body_text:
                            mail_item.Body = parsed.body_text
                        else:
                            mail_item.Body = ""

                        # Anexos
                        for att in parsed.attachments:
                            payload = att.get("payload")
                            if payload:
                                safe_name = att.get("filename") or "anexo.dat"
                                temp_att_path = temp_dir / f"att_{idx}_{safe_name}"
                                temp_att_path.write_bytes(payload)
                                try:
                                    mail_item.Attachments.Add(str(temp_att_path))
                                finally:
                                    if temp_att_path.exists():
                                        try:
                                            temp_att_path.unlink()
                                        except Exception:
                                            pass

                        mail_item.Save()
                        progress.success_count += 1

                    except Exception as msg_err:
                        progress.add_log(f"Falha ao gravar item no PST ({row['id']}): {msg_err}")
                        progress.error_count += 1

                    if idx % 10 == 0 or idx == total:
                        if on_progress:
                            on_progress(progress.to_dict())

                progress.status = "success" if progress.error_count == 0 else "partial"
                progress.add_log(
                    f"Exportação PST concluída! Sucesso: {progress.success_count}, Falhas: {progress.error_count}."
                )

            finally:
                # Desconecta a PST da sessão do Outlook de forma segura
                try:
                    ns.RemoveStore(root_folder)
                    progress.add_log("PST desconectado e salvo com sucesso.")
                except Exception as rem_err:
                    progress.add_log(f"Aviso ao desconectar PST do Outlook: {rem_err}")

                shutil.rmtree(temp_dir, ignore_errors=True)

        finally:
            pythoncom.CoUninitialize()

        if on_progress:
            on_progress(progress.to_dict())

        return progress.status == "success"
