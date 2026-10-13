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
from typing import Callable, List, Optional, Tuple
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


def format_internal_date(date_str: Optional[str]) -> Optional[str]:
    """Formata data para o padrão exigido pelo IMAP INTERNALDATE ('\"DD-Mmm-YYYY HH:MM:SS +HHMM\"')."""
    if not date_str:
        return None
    s = date_str.strip('"').strip()
    # Verifica se já está no formato de data interna do IMAP (ex: '24-Jun-2026 21:45:01 -0300')
    if re.match(r"^\d{1,2}-[A-Za-z]{3}-\d{4}\s+\d{2}:\d{2}:\d{2}\s+[+-]\d{4}$", s):
        return f'"{s}"'
    try:
        import imaplib
        dt = datetime.fromisoformat(s)
        return imaplib.Time2Internaldate(dt.timestamp())
    except Exception:
        pass
    try:
        from email.utils import parsedate_to_datetime
        import imaplib
        dt = parsedate_to_datetime(s)
        return imaplib.Time2Internaldate(dt.timestamp())
    except Exception:
        pass
    return None


def detect_target_namespace(client: ImapClient, dest_folders_list: List[dict]) -> Tuple[str, str]:
    """Detecta o prefixo de namespace (ex: 'INBOX.' ou '') e delimitador (ex: '.' ou '/') do destino."""
    prefix = ""
    delim = "/"

    # 1. Delimitador a partir da lista
    for f in dest_folders_list:
        d = f.get("delim")
        if d and d not in ('NIL', '""'):
            delim = d
            break

    # 2. Tenta comando IMAP NAMESPACE (RFC 2342)
    try:
        typ, dat = client.conn.namespace()
        if typ == "OK" and dat and dat[0]:
            raw = dat[0].decode("latin1", errors="replace") if isinstance(dat[0], bytes) else str(dat[0])
            m = re.search(r'\(\s*\(\s*"([^"]*)"\s+"([^"]+)"', raw)
            if m:
                p = m.group(1)
                d = m.group(2)
                if p:
                    prefix = p
                if d and d not in ('NIL', '""'):
                    delim = d
                return prefix, delim
    except Exception:
        pass

    # 3. Analisa se as pastas existentes (exceto INBOX) exigem prefixo INBOX.
    subfolders = [f["name"] for f in dest_folders_list if f["name"].upper() != "INBOX"]
    if subfolders and any(f.upper().startswith(f"INBOX{delim}") for f in subfolders):
        prefix = f"INBOX{delim}"

    return prefix, delim


def resolve_target_folder(
    source_folder: str,
    target_delim: str,
    target_prefix: Optional[str],
    existing_dest_map: dict,
    ns_prefix: str = ""
) -> str:
    """Determina o nome correspondente da pasta no destino, adaptando delimitadores, prefixos e namespace."""
    is_inbox = source_folder.upper() == "INBOX"

    # Se houver prefixo de restauração/exportação customizado (ex: Restaurado_Mailnov)
    if target_prefix:
        prefix_clean = target_prefix.strip("/. ")
        if is_inbox:
            base_folder = f"{prefix_clean}{target_delim}INBOX"
        else:
            sub = source_folder
            if sub.upper().startswith("INBOX.") or sub.upper().startswith("INBOX/"):
                sub = sub[6:]
            sub_conv = sub.replace("/", target_delim).replace(".", target_delim)
            base_folder = f"{prefix_clean}{target_delim}{sub_conv}"

        if ns_prefix and not base_folder.upper().startswith(ns_prefix.upper()):
            return f"{ns_prefix}{base_folder}"
        return base_folder

    # Sem prefixo customizado
    if is_inbox:
        return "INBOX"

    sub = source_folder
    if sub.upper().startswith("INBOX.") or sub.upper().startswith("INBOX/"):
        sub = sub[6:]

    sub_conv = sub.replace("/", target_delim).replace(".", target_delim)
    sub_lower = sub_conv.lower()

    # 1. Verifica se já existe diretamente no destino (ex: 'sent' ou 'trash')
    if sub_lower in existing_dest_map:
        return existing_dest_map[sub_lower]

    # 2. Verifica se existe com o namespace do destino (ex: 'inbox.sent' ou 'inbox.trash')
    if ns_prefix:
        ns_cand = f"{ns_prefix}{sub_conv}".lower()
        if ns_cand in existing_dest_map:
            return existing_dest_map[ns_cand]
    else:
        inbox_cand = f"inbox{target_delim}{sub_lower}"
        if inbox_cand in existing_dest_map:
            return existing_dest_map[inbox_cand]

    # 3. Mapeamento de pastas comuns se o destino tiver nomes correspondentes
    canon_map = {
        "sent": ["sent", "itens enviados", "enviados", "enviadas"],
        "trash": ["trash", "lixeira", "itens excluídos", "itens excluidos"],
        "drafts": ["drafts", "rascunhos"],
        "junk": ["junk", "spam", "lixo eletrônico", "lixo eletronico"],
        "spam": ["spam", "junk", "lixo eletrônico", "lixo eletronico"]
    }
    for orig_key, aliases in canon_map.items():
        if sub_lower in aliases:
            for alias in aliases:
                key_direct = alias
                key_ns = f"{ns_prefix}{alias}".lower() if ns_prefix else f"inbox{target_delim}{alias}".lower()
                if key_direct in existing_dest_map:
                    return existing_dest_map[key_direct]
                if key_ns in existing_dest_map:
                    return existing_dest_map[key_ns]

    # 4. Se o servidor de destino exige namespace (ex: 'INBOX.')
    if ns_prefix and not sub_conv.upper().startswith(ns_prefix.upper()):
        return f"{ns_prefix}{sub_conv}"

    return sub_conv


def ensure_imap_hierarchy(
    client: ImapClient,
    folder_path: str,
    delim: str,
    cache: set,
    progress: Optional[ExportProgress] = None
) -> bool:
    """Cria recursivamente todos os níveis de pasta no servidor IMAP de destino."""
    if not folder_path or folder_path.upper() == "INBOX":
        return True

    parts = [p.strip() for p in folder_path.split(delim) if p.strip()]
    if not parts:
        return True

    current = ""
    for part in parts:
        current = f"{current}{delim}{part}" if current else part
        current_lower = current.lower()
        if current_lower == "inbox" or current_lower in cache:
            continue

        encoded = encode_imap_utf7(current)
        try:
            folder_arg = f'"{encoded}"' if " " in encoded else encoded
            typ, resp = client.conn.create(folder_arg)
            resp_str = str(resp).upper()
            if typ == "OK":
                cache.add(current_lower)
                if progress:
                    progress.add_log(f"Pasta criada no destino: {current}")
            elif "ALREADYEXISTS" in resp_str or "EXISTS" in resp_str:
                cache.add(current_lower)
            else:
                alt_arg = encoded if folder_arg.startswith('"') else f'"{encoded}"'
                typ2, resp2 = client.conn.create(alt_arg)
                if typ2 == "OK" or "ALREADYEXISTS" in str(resp2).upper():
                    cache.add(current_lower)
                    if progress and typ2 == "OK":
                        progress.add_log(f"Pasta criada no destino: {current}")
                else:
                    cache.add(current_lower)
        except Exception:
            cache.add(current_lower)
    return True


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
            progress.status = "warning"
            filt_conta = req.profile_id or "Todas as contas"
            filt_pasta = req.folder or "Todas as pastas"
            progress.add_log(f"Atenção: Nenhuma mensagem encontrada no arquivo local para exportação.")
            progress.add_log(f"Filtro aplicado: Conta '{filt_conta}', Pasta '{filt_pasta}'.")
            progress.add_log("Dica: Verifique se realizou o backup de e-mails antes de tentar exportar.")
            if on_progress:
                on_progress(progress.to_dict())
            return False

        client = ImapClient(target_cfg)
        try:
            progress.add_log(f"Conectando ao servidor IMAP de destino: {req.target_host}:{req.target_port}...")
            client.connect()
            progress.add_log("Autenticação com o servidor de destino bem-sucedida.")

            # Descobre pastas existentes, delimitador e namespace no destino
            dest_folders_list = client.list_folders()
            existing_dest_folders = {f["name"].lower(): f["raw_name"] for f in dest_folders_list}

            ns_prefix, target_delim = detect_target_namespace(client, dest_folders_list)
            if ns_prefix:
                progress.add_log(f"Namespace detectado no servidor destino: '{ns_prefix}' (delimitador: '{target_delim}').")
            else:
                progress.add_log(f"Delimitador detectado no servidor destino: '{target_delim}'.")
            progress.add_log(f"Pastas existentes no destino: {len(dest_folders_list)}.")
            progress.add_log(f"Iniciando transferência de {total} mensagem(ns)...")

            created_folders_cache = {f["name"].lower() for f in dest_folders_list}
            created_folders_cache.add("inbox")

            for idx, msg in enumerate(messages, 1):
                if progress.is_cancelled:
                    progress.status = "cancelled"
                    progress.add_log("Migração IMAP cancelada pelo usuário.")
                    break

                progress.current_index = idx
                progress.percent = (idx / total) * 100
                progress.current_folder = msg["folder"]
                progress.current_subject = msg["subject"] or "(Sem assunto)"

                # Determina pasta de destino com tradução de hierarquia e namespace
                folder_dest = resolve_target_folder(
                    msg["folder"],
                    target_delim,
                    req.target_folder_prefix,
                    existing_dest_folders,
                    ns_prefix
                )

                # Garante que toda a árvore de pastas pai/filho exista
                ensure_imap_hierarchy(client, folder_dest, target_delim, created_folders_cache, progress)

                # Carrega bytes do arquivo .eml
                eml_abs = STORAGE_DIR / msg["eml_relative_path"]
                if not eml_abs.exists():
                    progress.add_log(f"Arquivo EML ausente no disco: {msg['eml_relative_path']}")
                    progress.error_count += 1
                    continue

                raw_bytes = eml_abs.read_bytes()

                # Prepara Flags
                flags_to_send = None
                if req.preserve_flags and msg["flags"]:
                    fl_list = [f for f in msg["flags"].split() if f.startswith("\\")]
                    if fl_list:
                        flags_to_send = f"({' '.join(fl_list)})"

                # Prepara InternalDate
                internal_date_val = None
                if req.preserve_internal_date:
                    internal_date_val = format_internal_date(msg["date_received"] or msg["date_sent"])

                # Argumento de pasta para APPEND
                if folder_dest.upper() == "INBOX":
                    folder_arg = "INBOX"
                else:
                    enc_f = encode_imap_utf7(folder_dest)
                    folder_arg = f'"{enc_f}"' if " " in enc_f or not enc_f.isalnum() else enc_f

                # APPEND no servidor
                try:
                    typ, resp = client.conn.append(
                        folder_arg,
                        flags_to_send,
                        internal_date_val,
                        raw_bytes
                    )

                    # Trata erro de namespace do Dovecot ("Client tried to access nonexistent namespace. (Mailbox name should probably be prefixed with: INBOX.)")
                    resp_str = str(resp).upper()
                    if typ != "OK" and ("NONEXISTENT NAMESPACE" in resp_str or "PREFIXED WITH: INBOX" in resp_str or "NAMESPACE" in resp_str):
                        ns_prefix = f"INBOX{target_delim}"
                        if not folder_dest.upper().startswith(ns_prefix.upper()):
                            folder_dest = f"{ns_prefix}{folder_dest}"
                        progress.add_log(f"Prefixo '{ns_prefix}' exigido pelo servidor. Reenviando mensagem #{msg['id']} para '{folder_dest}'...")
                        ensure_imap_hierarchy(client, folder_dest, target_delim, created_folders_cache, progress)
                        enc_f = encode_imap_utf7(folder_dest)
                        folder_arg = f'"{enc_f}"' if " " in enc_f or not enc_f.isalnum() else enc_f
                        typ, resp = client.conn.append(
                            folder_arg,
                            flags_to_send,
                            internal_date_val,
                            raw_bytes
                        )

                    # Se falhar com TRYCREATE, força criação recursiva e retenta
                    if typ != "OK":
                        resp_text = str(resp).upper()
                        if "TRYCREATE" in resp_text or "DOES NOT EXIST" in resp_text:
                            ensure_imap_hierarchy(client, folder_dest, target_delim, created_folders_cache, progress)
                            typ, resp = client.conn.append(
                                folder_arg,
                                flags_to_send,
                                internal_date_val,
                                raw_bytes
                            )

                    # Se ainda recusado e for subpasta, tenta salvar na INBOX como fallback de segurança
                    if typ != "OK" and folder_dest.upper() != "INBOX":
                        progress.add_log(f"Pasta '{folder_dest}' não aceitou mensagem #{msg['id']} ({resp}). Gravando na INBOX principal como fallback de segurança...")
                        typ_inbox, resp_inbox = client.conn.append(
                            "INBOX",
                            flags_to_send,
                            internal_date_val,
                            raw_bytes
                        )
                        if typ_inbox == "OK":
                            typ = "OK"
                            progress.add_log(f"Mensagem #{msg['id']} salva com sucesso na INBOX após fallback.")

                    if typ == "OK":
                        progress.success_count += 1
                    else:
                        progress.add_log(f"Erro ao enviar mensagem #{msg['id']} ('{msg['subject'][:30]}') para '{folder_dest}': {resp}")
                        progress.error_count += 1

                except Exception as app_err:
                    progress.add_log(f"Falha ao enviar mensagem #{msg['id']}: {app_err}")
                    progress.error_count += 1

                if idx % 10 == 0 or idx == total:
                    if on_progress:
                        on_progress(progress.to_dict())

            if progress.success_count == 0 and progress.error_count > 0:
                progress.status = "error"
                progress.add_log(
                    f"Exportação IMAP falhou integralmente: 0/{total} mensagens transferidas. {progress.error_count} erros registrados."
                )
            elif progress.error_count > 0:
                progress.status = "partial"
                progress.add_log(
                    f"Exportação IMAP concluída com avisos: {progress.success_count} mensagens enviadas, {progress.error_count} falhas."
                )
            else:
                progress.status = "success"
                progress.add_log(
                    f"Exportação IMAP finalizada com sucesso! Total de {progress.success_count} mensagem(ns) transferidas para a conta de destino."
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

        if total == 0:
            progress.status = "warning"
            progress.add_log("Atenção: Nenhuma mensagem encontrada no arquivo local para exportação.")
            if on_progress:
                on_progress(progress.to_dict())
            return False

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
        return progress.status == "success"

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

        if total == 0:
            progress.status = "warning"
            progress.add_log("Atenção: Nenhuma mensagem encontrada no arquivo local para exportação.")
            if on_progress:
                on_progress(progress.to_dict())
            return False

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
        return progress.status == "success"

