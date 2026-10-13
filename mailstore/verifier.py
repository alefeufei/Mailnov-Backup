"""Verificador e auditor de integridade do arquivo MailStore."""

import hashlib
from datetime import datetime
from pathlib import Path
from typing import Callable, Dict, List, Optional
from mailstore.config import STORAGE_DIR
from mailstore.database import Database


class IntegrityReport:
    def __init__(self):
        self.total_checked = 0
        self.valid_count = 0
        self.missing_count = 0
        self.corrupt_count = 0
        self.missing_files: List[str] = []
        self.corrupt_files: List[Dict[str, str]] = []
        self.status = "running"
        self.percent = 0.0
        self.started_at = datetime.now().isoformat()
        self.finished_at: Optional[str] = None

    def to_dict(self) -> dict:
        return {
            "total_checked": self.total_checked,
            "valid_count": self.valid_count,
            "missing_count": self.missing_count,
            "corrupt_count": self.corrupt_count,
            "missing_files": self.missing_files[:50],
            "corrupt_files": self.corrupt_files[:50],
            "status": self.status,
            "percent": round(self.percent, 1),
            "started_at": self.started_at,
            "finished_at": self.finished_at,
        }


class ArchiveVerifier:
    def __init__(self, db: Database):
        self.db = db

    def verify_archive(
        self,
        account_id: Optional[str] = None,
        on_progress: Optional[Callable[[dict], None]] = None
    ) -> IntegrityReport:
        report = IntegrityReport()

        with self.db.get_connection() as conn:
            sql = "SELECT id, account_id, folder, uid, sha256, size_bytes, eml_relative_path FROM messages"
            params = ()
            if account_id:
                sql += " WHERE account_id = ?"
                params = (account_id,)
            cur = conn.execute(sql, params)
            rows = cur.fetchall()

        total = len(rows)
        if total == 0:
            report.status = "success"
            report.finished_at = datetime.now().isoformat()
            if on_progress:
                on_progress(report.to_dict())
            return report

        for idx, row in enumerate(rows, 1):
            report.total_checked += 1
            report.percent = (idx / total) * 100

            eml_file = STORAGE_DIR / row["eml_relative_path"]
            if not eml_file.exists():
                report.missing_count += 1
                report.missing_files.append(row["eml_relative_path"])
                continue

            try:
                data = eml_file.read_bytes()
                current_hash = hashlib.sha256(data).hexdigest()
                if current_hash.lower() == row["sha256"].lower():
                    report.valid_count += 1
                else:
                    report.corrupt_count += 1
                    report.corrupt_files.append({
                        "file": row["eml_relative_path"],
                        "expected_hash": row["sha256"],
                        "actual_hash": current_hash,
                    })
            except Exception as e:
                report.corrupt_count += 1
                report.corrupt_files.append({
                    "file": row["eml_relative_path"],
                    "error": str(e)
                })

            if idx % 50 == 0 or idx == total:
                if on_progress:
                    on_progress(report.to_dict())

        report.finished_at = datetime.now().isoformat()
        report.status = "success" if (report.missing_count == 0 and report.corrupt_count == 0) else "warning"

        if on_progress:
            on_progress(report.to_dict())

        return report
