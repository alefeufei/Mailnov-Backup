"""Mecanismo de sincronização e comunicação IMAP resiliente."""

import imaplib
import re
import socket
import ssl
import time
from datetime import datetime
from typing import Callable, Dict, List, Optional, Set, Tuple
from mailstore.config import STORAGE_DIR
from mailstore.database import Database
from mailstore.models import BackupProfile, FolderInfo, ImapConfig
from mailstore.parser import ParsedEmail


def decode_imap_utf7(s: str) -> str:
    """Decodifica nomes de pastas no formato IMAP Modified UTF-7 (RFC 3501)."""
    if not s:
        return ""
    # Decodificação básica com fallback seguro
    try:
        import binascii
        def repl(match):
            b64_str = match.group(1).replace(",", "/")
            pad = len(b64_str) % 4
            if pad:
                b64_str += "=" * (4 - pad)
            try:
                raw = binascii.a2b_base64(b64_str)
                return raw.decode("utf-16-be")
            except Exception:
                return match.group(0)

        # IMAP Modified UTF-7 usa &...- para strings não ASCII
        decoded = re.sub(r"&([A-Za-z0-9+,]+)-", repl, s)
        decoded = decoded.replace("&-", "&")
        return decoded
    except Exception:
        return s


def encode_imap_utf7(s: str) -> str:
    """Codifica nomes de pastas para o formato IMAP Modified UTF-7."""
    if not s:
        return ""
    try:
        import binascii
        res = []
        buf = []
        for ch in s:
            if 0x20 <= ord(ch) <= 0x7e and ch != '&':
                if buf:
                    raw = "".join(buf).encode("utf-16-be")
                    b64 = binascii.b2a_base64(raw).decode("ascii").strip().replace("/", ",")
                    res.append(f"&{b64.rstrip('=')}-")
                    buf = []
                res.append(ch)
            elif ch == '&':
                if buf:
                    raw = "".join(buf).encode("utf-16-be")
                    b64 = binascii.b2a_base64(raw).decode("ascii").strip().replace("/", ",")
                    res.append(f"&{b64.rstrip('=')}-")
                    buf = []
                res.append("&-")
            else:
                buf.append(ch)
        if buf:
            raw = "".join(buf).encode("utf-16-be")
            b64 = binascii.b2a_base64(raw).decode("ascii").strip().replace("/", ",")
            res.append(f"&{b64.rstrip('=')}-")
        return "".join(res)
    except Exception:
        return s


def parse_list_response(line: str) -> Tuple[List[str], str, str]:
    """Interpreta linha da resposta LIST do IMAP (flags, delimitador, nome da pasta)."""
    # Exemplo: (\HasNoChildren) "/" "INBOX" ou (\HasChildren \Drafts) "." "Drafts"
    match = re.match(r'^\((?P<flags>[^)]*)\)\s+(?:"(?P<delim>[^"]+)"|(?P<delim_raw>\S+))\s+(?:"(?P<name>[^"]+)"|(?P<name_raw>.+))$', line.strip())
    if match:
        flags_str = match.group("flags")
        flags = [f.strip() for f in flags_str.split() if f.strip()]
        delim = match.group("delim") or match.group("delim_raw") or "/"
        name = match.group("name") or match.group("name_raw") or ""
        return flags, delim, name.strip()
    return [], "/", line.strip()


class ImapClient:
    def __init__(self, config: ImapConfig):
        self.config = config
        self.conn: Optional[imaplib.IMAP4] = None

    def connect(self) -> imaplib.IMAP4:
        """Estabelece a conexão IMAP de acordo com o modo SSL configurado."""
        host = self.config.host
        port = self.config.port
        ssl_mode = self.config.ssl_mode.upper()
        timeout = getattr(self.config, "timeout", 30)

        try:
            if ssl_mode == "SSL":
                ctx = ssl.create_default_context()
                conn = imaplib.IMAP4_SSL(host, port, ssl_context=ctx, timeout=timeout)
            elif ssl_mode == "STARTTLS":
                conn = imaplib.IMAP4(host, port, timeout=timeout)
                ctx = ssl.create_default_context()
                conn.starttls(ssl_context=ctx)
            else:  # PLAIN
                conn = imaplib.IMAP4(host, port, timeout=timeout)

            # Autenticação
            conn.login(self.config.username, self.config.password)
            self.conn = conn
            return conn
        except socket.timeout:
            raise ConnectionError(f"Tempo limite esgotado ao conectar a {host}:{port}")
        except ssl.SSLError as e:
            raise ConnectionError(f"Falha de certificado SSL/TLS com {host}: {e}")
        except imaplib.IMAP4.error as e:
            raise PermissionError(f"Erro de autenticação IMAP: {e}")
        except Exception as e:
            raise ConnectionError(f"Não foi possível conectar ao servidor IMAP {host}: {e}")

    def disconnect(self):
        if self.conn:
            try:
                self.conn.logout()
            except Exception:
                pass
            self.conn = None

    def list_folders(self) -> List[Dict[str, str]]:
        """Retorna todas as pastas disponíveis no servidor com nomes amigáveis."""
        if not self.conn:
            self.connect()

        typ, data = self.conn.list()
        if typ != "OK" or not data:
            return []

        folders = []
        for item in data:
            if not item:
                continue
            line = item.decode("latin1", errors="replace") if isinstance(item, bytes) else str(item)
            flags, delim, raw_name = parse_list_response(line)
            decoded_name = decode_imap_utf7(raw_name)
            folders.append({
                "raw_name": raw_name,
                "name": decoded_name,
                "delim": delim,
                "flags": flags,
            })
        return folders


def test_imap_connection(config: ImapConfig) -> Dict[str, any]:
    """Testa a conectividade com o servidor IMAP e retorna diagnóstico detalhado."""
    client = ImapClient(config)
    start_time = time.time()
    try:
        client.connect()
        folders = client.list_folders()
        duration = round(time.time() - start_time, 2)
        return {
            "success": True,
            "message": f"Conexão estabelecida com sucesso em {duration}s!",
            "folders_count": len(folders),
            "folders": [f["name"] for f in folders],
        }
    except Exception as e:
        return {
            "success": False,
            "message": str(e),
            "folders_count": 0,
            "folders": [],
        }
    finally:
        client.disconnect()


class BackupProgress:
    """Objeto para acompanhamento em tempo real de execuções de backup."""
    def __init__(self, profile_id: str):
        self.profile_id = profile_id
        self.current_folder = ""
        self.folder_index = 0
        self.total_folders = 0
        self.current_msg = 0
        self.total_msgs = 0
        self.synced_count = 0
        self.skipped_count = 0
        self.error_count = 0
        self.percent = 0.0
        self.speed_mps = 0.0
        self.status = "running"
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
            "profile_id": self.profile_id,
            "current_folder": self.current_folder,
            "folder_index": self.folder_index,
            "total_folders": self.total_folders,
            "current_msg": self.current_msg,
            "total_msgs": self.total_msgs,
            "synced_count": self.synced_count,
            "skipped_count": self.skipped_count,
            "error_count": self.error_count,
            "percent": round(self.percent, 1),
            "speed_mps": round(self.speed_mps, 1),
            "status": self.status,
            "logs": self.log_lines[-20:],
        }


class BackupRunner:
    """Executor central de arquivamento incremental."""
    def __init__(self, db: Database):
        self.db = db

    def _is_connection_loss(self, err: Exception) -> bool:
        err_msg = str(err).lower()
        return any(
            p in err_msg for p in [
                "eof occurred in violation of protocol",
                "broken pipe",
                "connection reset",
                "connection aborted",
                "connection closed",
                "socket error",
                "bad file descriptor",
                "timed out",
                "timeout",
                "abort: socket error"
            ]
        )

    def _select_folder_with_retry(
        self,
        client: ImapClient,
        folder_raw: str,
        folder_name: str,
        progress: BackupProgress,
        max_retries: int = 3
    ) -> bool:
        for attempt in range(1, max_retries + 1):
            try:
                typ, _ = client.conn.select(f'"{folder_raw}"', readonly=True)
                if typ == "OK":
                    return True
            except Exception as e:
                if self._is_connection_loss(e):
                    progress.add_log(f"Queda de conexão ao selecionar '{folder_name}'. Reconectando ({attempt}/{max_retries})...")
                    client.disconnect()
                    time.sleep(2 * attempt)
                    try:
                        client.connect()
                        continue
                    except Exception:
                        pass
            try:
                typ, _ = client.conn.select(folder_raw, readonly=True)
                if typ == "OK":
                    return True
            except Exception as e:
                if self._is_connection_loss(e):
                    progress.add_log(f"Queda de conexão ao selecionar '{folder_name}'. Reconectando ({attempt}/{max_retries})...")
                    client.disconnect()
                    time.sleep(2 * attempt)
                    try:
                        client.connect()
                        continue
                    except Exception:
                        pass
            time.sleep(1)
        return False

    def run_backup(
        self,
        profile: BackupProfile,
        progress: BackupProgress,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> bool:
        db = self.db
        client = ImapClient(profile.imap)
        started_at = datetime.now().isoformat()
        total_synced_this_run = 0
        total_errors_this_run = 0

        try:
            progress.add_log(f"Iniciando conexão com {profile.imap.host}:{profile.imap.port}...")
            client.connect()
            progress.add_log("Conexão IMAP autenticada com sucesso.")

            # Descobre pastas no servidor
            available_folders = client.list_folders()
            
            # Filtro de seleção de pastas
            selected_names = set(profile.selected_folders) if profile.selected_folders else None
            excluded_names = set(profile.exclude_folders) if profile.exclude_folders else set()

            folders_to_backup = []
            for f in available_folders:
                fname = f["name"]
                if excluded_names and fname in excluded_names:
                    continue
                if selected_names is not None and fname not in selected_names:
                    continue
                folders_to_backup.append(f)

            progress.total_folders = len(folders_to_backup)
            progress.add_log(f"{len(folders_to_backup)} pasta(s) selecionada(s) para arquivamento.")

            for idx, folder_info in enumerate(folders_to_backup, 1):
                if progress.is_cancelled:
                    progress.add_log("Operação cancelada pelo usuário.")
                    progress.status = "cancelled"
                    break

                folder_raw = folder_info["raw_name"]
                folder_name = folder_info["name"]
                delim = folder_info["delim"]

                progress.current_folder = folder_name
                progress.folder_index = idx
                progress.add_log(f"[{idx}/{progress.total_folders}] Analisando pasta: {folder_name}")

                db.upsert_folder(profile.id, folder_name, delim)

                # Seleciona a pasta com retry e reconexão automática
                selected_ok = self._select_folder_with_retry(client, folder_raw, folder_name, progress)
                if not selected_ok:
                    progress.add_log(f"Aviso: Não foi possível selecionar pasta '{folder_name}'. Pulando para a próxima...")
                    total_errors_this_run += 1
                    continue

                # Busca UIDs no servidor
                search_criteria = ["ALL"]
                if profile.since_date:
                    try:
                        d = datetime.strptime(profile.since_date, "%Y-%m-%d")
                        imap_date = d.strftime("%d-%b-%Y")
                        search_criteria = ["SINCE", imap_date]
                    except Exception:
                        pass

                try:
                    typ, search_data = client.conn.uid("SEARCH", *search_criteria)
                except Exception as search_err:
                    if self._is_connection_loss(search_err):
                        progress.add_log(f"Reconectando para pesquisar mensagens em '{folder_name}'...")
                        client.disconnect()
                        time.sleep(2)
                        client.connect()
                        self._select_folder_with_retry(client, folder_raw, folder_name, progress)
                        typ, search_data = client.conn.uid("SEARCH", *search_criteria)
                    else:
                        raise search_err

                if typ != "OK" or not search_data or not search_data[0]:
                    progress.add_log(f"Pasta '{folder_name}' está vazia.")
                    db.update_folder_stats(profile.id, folder_name, 0)
                    continue

                server_uids = [int(u) for u in search_data[0].split() if u.isdigit()]
                existing_uids = db.get_existing_uids(profile.id, folder_name)

                # Delta incremental: apenas mensagens novas
                new_uids = sorted(list(set(server_uids) - existing_uids))
                skipped = len(server_uids) - len(new_uids)
                progress.skipped_count += skipped
                progress.total_msgs = len(server_uids)
                progress.current_msg = skipped

                progress.add_log(
                    f"'{folder_name}': {len(server_uids)} no servidor, {len(new_uids)} novas, {skipped} já arquivadas."
                )

                if on_progress:
                    on_progress(progress.to_dict())

                max_uid_seen = max(server_uids) if server_uids else 0

                # Download das mensagens novas com reconexão automática resiliente
                for i, uid in enumerate(new_uids, 1):
                    if progress.is_cancelled:
                        break

                    progress.current_msg = skipped + i
                    progress.percent = (progress.current_msg / max(1, progress.total_msgs)) * 100
                    elapsed = max(0.1, time.time() - progress.start_time)
                    progress.speed_mps = progress.synced_count / elapsed

                    # Pacing leve para evitar bloqueio anti-flood de servidores IMAP
                    if i > 1 and i % 50 == 0:
                        time.sleep(0.05)

                    fetch_data = None
                    for attempt in range(1, 4):
                        try:
                            typ, fetch_data = client.conn.uid(
                                "FETCH", str(uid), "(RFC822 FLAGS INTERNALDATE)"
                            )
                            if typ == "OK" and fetch_data:
                                break
                        except Exception as fetch_err:
                            if self._is_connection_loss(fetch_err):
                                progress.add_log(
                                    f"Conexão interrompida no UID {uid} ({fetch_err}). Reconectando automaticamente (tentativa {attempt}/3)..."
                                )
                                client.disconnect()
                                time.sleep(2 * attempt)
                                try:
                                    client.connect()
                                    self._select_folder_with_retry(client, folder_raw, folder_name, progress, 2)
                                    continue
                                except Exception:
                                    pass
                            else:
                                progress.add_log(f"Erro ao obter UID {uid} na pasta {folder_name}: {fetch_err}")
                                break

                    if not fetch_data:
                        progress.add_log(f"Falha definitiva ao obter UID {uid} na pasta {folder_name}.")
                        total_errors_this_run += 1
                        progress.error_count += 1
                        continue

                    # Extrai bytes do RFC822 e metadados
                    raw_bytes = None
                    flags_str = ""
                    internal_date = None

                    try:
                        for part in fetch_data:
                            if isinstance(part, tuple):
                                header_info = part[0].decode("latin1", errors="replace")
                                fl_match = re.search(r"FLAGS \(([^)]*)\)", header_info)
                                if fl_match:
                                    flags_str = fl_match.group(1)
                                id_match = re.search(r'INTERNALDATE "([^"]+)"', header_info)
                                if id_match:
                                    internal_date = id_match.group(1)
                                raw_bytes = part[1]
                                break

                        if not raw_bytes:
                            progress.add_log(f"UID {uid} não possui corpo RFC822 válido.")
                            continue

                        # Parse e persistência
                        parsed = ParsedEmail(
                            raw_bytes=raw_bytes,
                            account_id=profile.id,
                            folder=folder_name,
                            uid=uid,
                            flags=flags_str,
                            internal_date=internal_date
                        )

                        eml_path = parsed.save_eml(STORAGE_DIR)

                        db.insert_message(
                            account_id=profile.id,
                            folder=folder_name,
                            uid=uid,
                            message_id=parsed.message_id,
                            subject=parsed.subject,
                            sender=parsed.sender,
                            recipients=parsed.recipients,
                            cc=parsed.cc,
                            bcc=parsed.bcc,
                            reply_to=parsed.reply_to,
                            date_sent=parsed.date_sent,
                            date_received=parsed.date_received,
                            size_bytes=parsed.size_bytes,
                            flags=flags_str,
                            has_attachments=bool(parsed.attachments),
                            body_preview=parsed.body_preview,
                            body_text_full=parsed.body_text or clean_html_to_text(parsed.body_html),
                            sha256=parsed.sha256,
                            eml_relative_path=eml_path,
                            attachments=parsed.attachments
                        )

                        progress.synced_count += 1
                        total_synced_this_run += 1

                        if i % 10 == 0 or i == len(new_uids):
                            if on_progress:
                                on_progress(progress.to_dict())

                    except Exception as parse_err:
                        progress.add_log(f"Falha ao persistir mensagem UID {uid}: {parse_err}")
                        total_errors_this_run += 1
                        progress.error_count += 1

                # Atualiza estatísticas da pasta
                db.update_folder_stats(profile.id, folder_name, max_uid_seen)

            # Finalização
            finished_at = datetime.now().isoformat()
            if progress.is_cancelled:
                final_status = "cancelled"
            elif total_errors_this_run > 0:
                final_status = "partial"
            else:
                final_status = "success"

            progress.status = final_status
            progress.add_log(
                f"Backup finalizado. Status: {final_status}. Mensagens sincronizadas: {total_synced_this_run}, Falhas: {total_errors_this_run}."
            )

            db.update_profile_stats(profile.id, final_status, finished_at)
            db.log_sync(
                profile_id=profile.id,
                action="backup",
                status=final_status,
                messages_count=total_synced_this_run,
                errors_count=total_errors_this_run,
                started_at=started_at,
                finished_at=finished_at,
                details=f"Pastas: {progress.total_folders}, Sincronizadas: {total_synced_this_run}, Ignoradas: {progress.skipped_count}"
            )

            if on_progress:
                on_progress(progress.to_dict())

            return final_status == "success"

        except Exception as e:
            finished_at = datetime.now().isoformat()
            progress.status = "error"
            progress.add_log(f"Erro fatal durante o backup: {e}")
            db.update_profile_stats(profile.id, "error", finished_at)
            db.log_sync(
                profile_id=profile.id,
                action="backup",
                status="error",
                messages_count=total_synced_this_run,
                errors_count=total_errors_this_run + 1,
                started_at=started_at,
                finished_at=finished_at,
                details=f"Exceção: {str(e)}"
            )
            if on_progress:
                on_progress(progress.to_dict())
            return False

        finally:
            client.disconnect()
