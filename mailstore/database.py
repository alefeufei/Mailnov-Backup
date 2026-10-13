"""Camada de banco de dados SQLite com suporte a FTS5 e transações seguras."""

import json
import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Dict, List, Optional, Tuple
from mailstore.config import DB_PATH
from mailstore.models import (
    AttachmentInfo,
    BackupProfile,
    EmailDetail,
    EmailSummary,
    FolderInfo,
    ImapConfig,
    SearchQuery,
)


class Database:
    def __init__(self, db_path: Optional[Path] = None):
        self.db_path = db_path or DB_PATH
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        self.init_schema()

    @contextmanager
    def get_connection(self):
        conn = sqlite3.connect(str(self.db_path), timeout=30.0)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys = ON;")
        conn.execute("PRAGMA synchronous = NORMAL;")
        try:
            yield conn
        finally:
            conn.close()

    def init_schema(self):
        with self.get_connection() as conn:
            # Tabela de perfis de backup
            conn.execute("""
                CREATE TABLE IF NOT EXISTS profiles (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    host TEXT NOT NULL,
                    port INTEGER NOT NULL,
                    ssl_mode TEXT NOT NULL,
                    username TEXT NOT NULL,
                    password TEXT NOT NULL,
                    selected_folders TEXT,
                    exclude_folders TEXT,
                    since_date TEXT,
                    created_at TEXT NOT NULL,
                    last_run TEXT,
                    last_status TEXT DEFAULT 'idle',
                    message_count INTEGER DEFAULT 0,
                    total_size_bytes INTEGER DEFAULT 0
                );
            """)

            # Tabela de pastas sincronizadas
            conn.execute("""
                CREATE TABLE IF NOT EXISTS folders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    account_id TEXT NOT NULL,
                    folder_name TEXT NOT NULL,
                    delimiter TEXT DEFAULT '/',
                    message_count INTEGER DEFAULT 0,
                    last_uid INTEGER DEFAULT 0,
                    UNIQUE(account_id, folder_name)
                );
            """)

            # Tabela principal de mensagens
            conn.execute("""
                CREATE TABLE IF NOT EXISTS messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    account_id TEXT NOT NULL,
                    folder TEXT NOT NULL,
                    uid INTEGER NOT NULL,
                    message_id TEXT,
                    subject TEXT,
                    sender TEXT,
                    recipients TEXT,
                    cc TEXT DEFAULT '',
                    bcc TEXT DEFAULT '',
                    reply_to TEXT DEFAULT '',
                    date_sent TEXT,
                    date_received TEXT,
                    size_bytes INTEGER DEFAULT 0,
                    flags TEXT DEFAULT '',
                    has_attachments INTEGER DEFAULT 0,
                    body_preview TEXT DEFAULT '',
                    body_text_full TEXT DEFAULT '',
                    sha256 TEXT NOT NULL,
                    eml_relative_path TEXT NOT NULL,
                    UNIQUE(account_id, folder, uid)
                );
            """)

            # Índices de performance
            conn.execute("CREATE INDEX IF NOT EXISTS idx_msg_account ON messages(account_id);")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_msg_folder ON messages(account_id, folder);")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_msg_sha256 ON messages(sha256);")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_msg_date ON messages(date_sent);")

            # Tabela de anexos
            conn.execute("""
                CREATE TABLE IF NOT EXISTS attachments (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    message_id INTEGER NOT NULL,
                    filename TEXT NOT NULL,
                    content_type TEXT DEFAULT '',
                    size_bytes INTEGER DEFAULT 0,
                    part_index INTEGER DEFAULT 0,
                    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
                );
            """)
            conn.execute("CREATE INDEX IF NOT EXISTS idx_att_msg ON attachments(message_id);")
            conn.execute("CREATE INDEX IF NOT EXISTS idx_att_name ON attachments(filename);")

            # Tabela FTS5 para busca em texto completo
            try:
                conn.execute("""
                    CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                        subject,
                        sender,
                        recipients,
                        body_preview,
                        body_text_full,
                        content='messages',
                        content_rowid='id'
                    );
                """)

                # Triggers de sincronização com FTS5
                conn.execute("""
                    CREATE TRIGGER IF NOT EXISTS msg_ai AFTER INSERT ON messages BEGIN
                        INSERT INTO messages_fts(rowid, subject, sender, recipients, body_preview, body_text_full)
                        VALUES (new.id, new.subject, new.sender, new.recipients, new.body_preview, new.body_text_full);
                    END;
                """)
                conn.execute("""
                    CREATE TRIGGER IF NOT EXISTS msg_ad AFTER DELETE ON messages BEGIN
                        INSERT INTO messages_fts(messages_fts, rowid, subject, sender, recipients, body_preview, body_text_full)
                        VALUES('delete', old.id, old.subject, old.sender, old.recipients, old.body_preview, old.body_text_full);
                    END;
                """)
                conn.execute("""
                    CREATE TRIGGER IF NOT EXISTS msg_au AFTER UPDATE ON messages BEGIN
                        INSERT INTO messages_fts(messages_fts, rowid, subject, sender, recipients, body_preview, body_text_full)
                        VALUES('delete', old.id, old.subject, old.sender, old.recipients, old.body_preview, old.body_text_full);
                        INSERT INTO messages_fts(rowid, subject, sender, recipients, body_preview, body_text_full)
                        VALUES (new.id, new.subject, new.sender, new.recipients, new.body_preview, new.body_text_full);
                    END;
                """)
            except Exception as e:
                # FTS5 pode não estar disponível em distribuições SQLite antigas, fallback gracioso
                pass

            # Tabela de histórico e logs de sincronização
            conn.execute("""
                CREATE TABLE IF NOT EXISTS sync_logs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    profile_id TEXT NOT NULL,
                    action TEXT NOT NULL,
                    status TEXT NOT NULL,
                    messages_count INTEGER DEFAULT 0,
                    errors_count INTEGER DEFAULT 0,
                    started_at TEXT NOT NULL,
                    finished_at TEXT,
                    details TEXT
                );
            """)
            conn.commit()

    # --- Métodos de Perfis ---
    def save_profile(self, profile: BackupProfile):
        with self.get_connection() as conn:
            conn.execute("""
                INSERT INTO profiles (
                    id, name, host, port, ssl_mode, username, password,
                    selected_folders, exclude_folders, since_date,
                    created_at, last_run, last_status, message_count, total_size_bytes
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    name=excluded.name,
                    host=excluded.host,
                    port=excluded.port,
                    ssl_mode=excluded.ssl_mode,
                    username=excluded.username,
                    password=excluded.password,
                    selected_folders=excluded.selected_folders,
                    exclude_folders=excluded.exclude_folders,
                    since_date=excluded.since_date,
                    last_run=excluded.last_run,
                    last_status=excluded.last_status,
                    message_count=excluded.message_count,
                    total_size_bytes=excluded.total_size_bytes;
            """, (
                profile.id,
                profile.name,
                profile.imap.host,
                profile.imap.port,
                profile.imap.ssl_mode,
                profile.imap.username,
                profile.imap.password,
                json.dumps(profile.selected_folders),
                json.dumps(profile.exclude_folders),
                profile.since_date,
                profile.created_at,
                profile.last_run,
                profile.last_status,
                profile.message_count,
                profile.total_size_bytes
            ))
            conn.commit()

    def get_profile(self, profile_id: str) -> Optional[BackupProfile]:
        with self.get_connection() as conn:
            cur = conn.execute("SELECT * FROM profiles WHERE id = ?", (profile_id,))
            row = cur.fetchone()
            if not row:
                return None
            return BackupProfile(
                id=row["id"],
                name=row["name"],
                imap=ImapConfig(
                    host=row["host"],
                    port=row["port"],
                    ssl_mode=row["ssl_mode"],
                    username=row["username"],
                    password=row["password"]
                ),
                selected_folders=json.loads(row["selected_folders"] or "[]"),
                exclude_folders=json.loads(row["exclude_folders"] or "[]"),
                since_date=row["since_date"],
                created_at=row["created_at"],
                last_run=row["last_run"],
                last_status=row["last_status"],
                message_count=row["message_count"] or 0,
                total_size_bytes=row["total_size_bytes"] or 0
            )

    def list_profiles(self) -> List[BackupProfile]:
        profiles = []
        with self.get_connection() as conn:
            cur = conn.execute("SELECT * FROM profiles ORDER BY name ASC")
            for row in cur.fetchall():
                profiles.append(BackupProfile(
                    id=row["id"],
                    name=row["name"],
                    imap=ImapConfig(
                        host=row["host"],
                        port=row["port"],
                        ssl_mode=row["ssl_mode"],
                        username=row["username"],
                        password=row["password"]
                    ),
                    selected_folders=json.loads(row["selected_folders"] or "[]"),
                    exclude_folders=json.loads(row["exclude_folders"] or "[]"),
                    since_date=row["since_date"],
                    created_at=row["created_at"],
                    last_run=row["last_run"],
                    last_status=row["last_status"],
                    message_count=row["message_count"] or 0,
                    total_size_bytes=row["total_size_bytes"] or 0
                ))
        return profiles

    def delete_profile(self, profile_id: str):
        with self.get_connection() as conn:
            conn.execute("DELETE FROM messages WHERE account_id = ?", (profile_id,))
            conn.execute("DELETE FROM folders WHERE account_id = ?", (profile_id,))
            conn.execute("DELETE FROM sync_logs WHERE profile_id = ?", (profile_id,))
            conn.execute("DELETE FROM profiles WHERE id = ?", (profile_id,))
            conn.commit()

    def update_profile_stats(self, profile_id: str, last_status: str, last_run: Optional[str] = None):
        with self.get_connection() as conn:
            count_cur = conn.execute(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM messages WHERE account_id = ?",
                (profile_id,)
            )
            count, size = count_cur.fetchone()
            if last_run:
                conn.execute("""
                    UPDATE profiles SET
                        last_status = ?,
                        last_run = ?,
                        message_count = ?,
                        total_size_bytes = ?
                    WHERE id = ?
                """, (last_status, last_run, count, size, profile_id))
            else:
                conn.execute("""
                    UPDATE profiles SET
                        last_status = ?,
                        message_count = ?,
                        total_size_bytes = ?
                    WHERE id = ?
                """, (last_status, count, size, profile_id))
            conn.commit()

    # --- Pastas ---
    def upsert_folder(self, account_id: str, folder_name: str, delimiter: str = "/"):
        with self.get_connection() as conn:
            conn.execute("""
                INSERT INTO folders (account_id, folder_name, delimiter, message_count)
                VALUES (?, ?, ?, 0)
                ON CONFLICT(account_id, folder_name) DO UPDATE SET delimiter = excluded.delimiter;
            """, (account_id, folder_name, delimiter))
            conn.commit()

    def update_folder_stats(self, account_id: str, folder_name: str, last_uid: int):
        with self.get_connection() as conn:
            count_cur = conn.execute(
                "SELECT COUNT(*) FROM messages WHERE account_id = ? AND folder = ?",
                (account_id, folder_name)
            )
            count = count_cur.fetchone()[0]
            conn.execute("""
                UPDATE folders SET message_count = ?, last_uid = ?
                WHERE account_id = ? AND folder_name = ?
            """, (count, last_uid, account_id, folder_name))
            conn.commit()

    def get_folders(self, account_id: Optional[str] = None) -> List[FolderInfo]:
        results = []
        with self.get_connection() as conn:
            sql = "SELECT account_id, folder_name, delimiter, message_count, last_uid FROM folders"
            params = ()
            if account_id:
                sql += " WHERE account_id = ? ORDER BY folder_name ASC"
                params = (account_id,)
            else:
                sql += " ORDER BY account_id ASC, folder_name ASC"

            cur = conn.execute(sql, params)
            for row in cur.fetchall():
                results.append(FolderInfo(
                    account_id=row["account_id"],
                    folder_name=row["folder_name"],
                    delimiter=row["delimiter"],
                    message_count=row["message_count"] or 0,
                    last_uid=row["last_uid"] or 0
                ))
        return results

    def get_browse_tree(self) -> List[dict]:
        """Retorna a estrutura hierárquica completa agrupada por perfil e suas respectivas pastas."""
        profiles = self.list_profiles()
        prof_map = {p.id: p for p in profiles}

        all_folders = self.get_folders()
        tree = []

        folders_by_acc: Dict[str, List[FolderInfo]] = {}
        for f in all_folders:
            folders_by_acc.setdefault(f.account_id, []).append(f)

        for prof in profiles:
            p_folders = folders_by_acc.get(prof.id, [])
            tree.append({
                "account_id": prof.id,
                "account_name": prof.name,
                "username": prof.imap.username,
                "host": prof.imap.host,
                "total_messages": prof.message_count,
                "total_size_bytes": prof.total_size_bytes,
                "folders": [
                    {
                        "folder_name": f.folder_name,
                        "delimiter": f.delimiter,
                        "message_count": f.message_count,
                    }
                    for f in p_folders
                ]
            })

        for acc_id, p_folders in folders_by_acc.items():
            if acc_id not in prof_map:
                total_msgs = sum(f.message_count for f in p_folders)
                tree.append({
                    "account_id": acc_id,
                    "account_name": f"Conta {acc_id}",
                    "username": acc_id,
                    "host": "",
                    "total_messages": total_msgs,
                    "total_size_bytes": 0,
                    "folders": [
                        {
                            "folder_name": f.folder_name,
                            "delimiter": f.delimiter,
                            "message_count": f.message_count,
                        }
                        for f in p_folders
                    ]
                })

        return tree

    # --- Mensagens ---
    def get_existing_uids(self, account_id: str, folder: str) -> set:
        with self.get_connection() as conn:
            cur = conn.execute(
                "SELECT uid FROM messages WHERE account_id = ? AND folder = ?",
                (account_id, folder)
            )
            return {row[0] for row in cur.fetchall()}

    def get_max_uid(self, account_id: str, folder: str) -> int:
        with self.get_connection() as conn:
            cur = conn.execute(
                "SELECT COALESCE(MAX(uid), 0) FROM messages WHERE account_id = ? AND folder = ?",
                (account_id, folder)
            )
            return cur.fetchone()[0]

    def insert_message(
        self,
        account_id: str,
        folder: str,
        uid: int,
        message_id: str,
        subject: str,
        sender: str,
        recipients: str,
        cc: str,
        bcc: str,
        reply_to: str,
        date_sent: Optional[str],
        date_received: Optional[str],
        size_bytes: int,
        flags: str,
        has_attachments: bool,
        body_preview: str,
        body_text_full: str,
        sha256: str,
        eml_relative_path: str,
        attachments: List[dict]
    ) -> int:
        with self.get_connection() as conn:
            cur = conn.execute("""
                INSERT INTO messages (
                    account_id, folder, uid, message_id, subject, sender,
                    recipients, cc, bcc, reply_to, date_sent, date_received,
                    size_bytes, flags, has_attachments, body_preview,
                    body_text_full, sha256, eml_relative_path
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(account_id, folder, uid) DO UPDATE SET
                    flags=excluded.flags,
                    size_bytes=excluded.size_bytes
                RETURNING id;
            """, (
                account_id, folder, uid, message_id, subject, sender,
                recipients, cc, bcc, reply_to, date_sent, date_received,
                size_bytes, flags, 1 if has_attachments else 0,
                body_preview, body_text_full, sha256, eml_relative_path
            ))
            msg_id = cur.fetchone()[0]

            if attachments:
                for att in attachments:
                    conn.execute("""
                        INSERT INTO attachments (
                            message_id, filename, content_type, size_bytes, part_index
                        ) VALUES (?, ?, ?, ?, ?)
                    """, (
                        msg_id,
                        att.get("filename", "sem_nome"),
                        att.get("content_type", "application/octet-stream"),
                        att.get("size_bytes", 0),
                        att.get("part_index", 0)
                    ))

            conn.commit()
            return msg_id

    def get_message_detail(self, message_id: int) -> Optional[EmailDetail]:
        with self.get_connection() as conn:
            cur = conn.execute("SELECT * FROM messages WHERE id = ?", (message_id,))
            row = cur.fetchone()
            if not row:
                return None

            # Buscar anexos
            att_cur = conn.execute(
                "SELECT * FROM attachments WHERE message_id = ? ORDER BY part_index ASC",
                (message_id,)
            )
            attachments = [
                AttachmentInfo(
                    id=r["id"],
                    message_id=r["message_id"],
                    filename=r["filename"],
                    content_type=r["content_type"],
                    size_bytes=r["size_bytes"],
                    part_index=r["part_index"]
                )
                for r in att_cur.fetchall()
            ]

            return EmailDetail(
                id=row["id"],
                account_id=row["account_id"],
                folder=row["folder"],
                uid=row["uid"],
                message_id=row["message_id"] or "",
                subject=row["subject"] or "",
                sender=row["sender"] or "",
                recipients=row["recipients"] or "",
                cc=row["cc"] or "",
                bcc=row["bcc"] or "",
                reply_to=row["reply_to"] or "",
                date_sent=row["date_sent"],
                date_received=row["date_received"],
                size_bytes=row["size_bytes"] or 0,
                flags=row["flags"] or "",
                has_attachments=bool(row["has_attachments"]),
                body_preview=row["body_preview"] or "",
                body_text=row["body_text_full"] or "",
                sha256=row["sha256"],
                attachments=attachments,
                eml_relative_path=row["eml_relative_path"]
            )

    def list_messages(
        self,
        account_id: Optional[str] = None,
        folder: Optional[str] = None,
        limit: int = 50,
        offset: int = 0
    ) -> Tuple[List[EmailSummary], int]:
        with self.get_connection() as conn:
            where_clauses = []
            params = []
            if account_id:
                where_clauses.append("account_id = ?")
                params.append(account_id)
            if folder:
                where_clauses.append("folder = ?")
                params.append(folder)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""

            # Contagem total
            count_cur = conn.execute(f"SELECT COUNT(*) FROM messages{where_sql}", tuple(params))
            total = count_cur.fetchone()[0]

            # Busca paginada
            sql = f"""
                SELECT id, account_id, folder, uid, message_id, subject, sender,
                       recipients, date_sent, date_received, size_bytes, flags,
                       has_attachments, body_preview, sha256
                FROM messages
                {where_sql}
                ORDER BY date_sent DESC, id DESC
                LIMIT ? OFFSET ?
            """
            cur = conn.execute(sql, tuple(params + [limit, offset]))
            items = []
            for row in cur.fetchall():
                items.append(EmailSummary(
                    id=row["id"],
                    account_id=row["account_id"],
                    folder=row["folder"],
                    uid=row["uid"],
                    message_id=row["message_id"] or "",
                    subject=row["subject"] or "",
                    sender=row["sender"] or "",
                    recipients=row["recipients"] or "",
                    date_sent=row["date_sent"],
                    date_received=row["date_received"],
                    size_bytes=row["size_bytes"] or 0,
                    flags=row["flags"] or "",
                    has_attachments=bool(row["has_attachments"]),
                    body_preview=row["body_preview"] or "",
                    sha256=row["sha256"]
                ))
            return items, total

    def search_messages(self, q: SearchQuery) -> Tuple[List[EmailSummary], int]:
        with self.get_connection() as conn:
            where_clauses = []
            params = []

            # Se houver termo geral de busca, usa FTS5 quando possível
            fts_matched_ids = None
            if q.query and q.query.strip():
                clean_term = q.query.replace('"', '""').strip()
                try:
                    cur_fts = conn.execute(
                        "SELECT rowid FROM messages_fts WHERE messages_fts MATCH ? LIMIT 1000",
                        (f'"{clean_term}"*',)
                    )
                    fts_matched_ids = [r[0] for r in cur_fts.fetchall()]
                except Exception:
                    # Fallback com LIKE tradicional
                    where_clauses.append("(subject LIKE ? OR sender LIKE ? OR recipients LIKE ? OR body_preview LIKE ?)")
                    like_term = f"%{clean_term}%"
                    params.extend([like_term, like_term, like_term, like_term])

            if fts_matched_ids is not None:
                if not fts_matched_ids:
                    return [], 0
                placeholders = ",".join("?" for _ in fts_matched_ids)
                where_clauses.append(f"id IN ({placeholders})")
                params.extend(fts_matched_ids)

            if q.account_id:
                where_clauses.append("account_id = ?")
                params.append(q.account_id)
            if q.folder:
                where_clauses.append("folder = ?")
                params.append(q.folder)
            if q.sender:
                where_clauses.append("sender LIKE ?")
                params.append(f"%{q.sender}%")
            if q.recipient:
                where_clauses.append("recipients LIKE ?")
                params.append(f"%{q.recipient}%")
            if q.subject:
                where_clauses.append("subject LIKE ?")
                params.append(f"%{q.subject}%")
            if q.has_attachment is not None:
                where_clauses.append("has_attachments = ?")
                params.append(1 if q.has_attachment else 0)
            if q.date_from:
                where_clauses.append("date_sent >= ?")
                params.append(q.date_from)
            if q.date_to:
                where_clauses.append("date_sent <= ?")
                params.append(q.date_to)

            where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""

            count_cur = conn.execute(f"SELECT COUNT(*) FROM messages{where_sql}", tuple(params))
            total = count_cur.fetchone()[0]

            sql = f"""
                SELECT id, account_id, folder, uid, message_id, subject, sender,
                       recipients, date_sent, date_received, size_bytes, flags,
                       has_attachments, body_preview, sha256
                FROM messages
                {where_sql}
                ORDER BY date_sent DESC, id DESC
                LIMIT ? OFFSET ?
            """
            cur = conn.execute(sql, tuple(params + [q.limit, q.offset]))
            items = []
            for row in cur.fetchall():
                items.append(EmailSummary(
                    id=row["id"],
                    account_id=row["account_id"],
                    folder=row["folder"],
                    uid=row["uid"],
                    message_id=row["message_id"] or "",
                    subject=row["subject"] or "",
                    sender=row["sender"] or "",
                    recipients=row["recipients"] or "",
                    date_sent=row["date_sent"],
                    date_received=row["date_received"],
                    size_bytes=row["size_bytes"] or 0,
                    flags=row["flags"] or "",
                    has_attachments=bool(row["has_attachments"]),
                    body_preview=row["body_preview"] or "",
                    sha256=row["sha256"]
                ))
            return items, total

    def log_sync(
        self,
        profile_id: str,
        action: str,
        status: str,
        messages_count: int,
        errors_count: int,
        started_at: str,
        finished_at: str,
        details: str
    ):
        with self.get_connection() as conn:
            conn.execute("""
                INSERT INTO sync_logs (
                    profile_id, action, status, messages_count,
                    errors_count, started_at, finished_at, details
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            """, (
                profile_id, action, status, messages_count,
                errors_count, started_at, finished_at, details
            ))
            conn.commit()

    def get_logs(self, limit: int = 50) -> List[dict]:
        with self.get_connection() as conn:
            cur = conn.execute("SELECT * FROM sync_logs ORDER BY id DESC LIMIT ?", (limit,))
            return [dict(row) for row in cur.fetchall()]

    def get_system_stats(self) -> dict:
        with self.get_connection() as conn:
            prof_count = conn.execute("SELECT COUNT(*) FROM profiles").fetchone()[0]
            msg_count = conn.execute("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM messages").fetchone()
            folder_count = conn.execute("SELECT COUNT(*) FROM folders").fetchone()[0]
            return {
                "profiles_count": prof_count,
                "total_messages": msg_count[0],
                "total_size_bytes": msg_count[1],
                "folders_count": folder_count,
            }
