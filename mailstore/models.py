"""Modelos de dados Pydantic do MailStore Backup."""

from datetime import datetime
from typing import List, Optional
from pydantic import BaseModel, Field


class ImapConfig(BaseModel):
    host: str
    port: int = 993
    ssl_mode: str = "SSL"  # "SSL", "STARTTLS", "PLAIN"
    username: str
    password: str
    timeout: int = 30


class BackupProfile(BaseModel):
    id: str
    name: str
    imap: ImapConfig
    selected_folders: List[str] = Field(default_factory=list)
    exclude_folders: List[str] = Field(default_factory=list)
    since_date: Optional[str] = None  # Formato YYYY-MM-DD
    created_at: str = Field(default_factory=lambda: datetime.now().isoformat())
    last_run: Optional[str] = None
    last_status: Optional[str] = "idle"  # "idle", "running", "success", "error"
    message_count: int = 0
    total_size_bytes: int = 0


class ProfileCreateRequest(BaseModel):
    name: str
    imap: ImapConfig
    selected_folders: List[str] = Field(default_factory=list)
    exclude_folders: List[str] = Field(default_factory=list)
    since_date: Optional[str] = None


class ProfileUpdateRequest(BaseModel):
    name: Optional[str] = None
    imap: Optional[ImapConfig] = None
    selected_folders: Optional[List[str]] = None
    exclude_folders: Optional[List[str]] = None
    since_date: Optional[str] = None


class FolderInfo(BaseModel):
    account_id: str
    folder_name: str
    delimiter: str = "/"
    attributes: List[str] = Field(default_factory=list)
    message_count: int = 0
    last_uid: int = 0


class AttachmentInfo(BaseModel):
    id: int
    message_id: int
    filename: str
    content_type: str
    size_bytes: int
    part_index: int = 0


class EmailSummary(BaseModel):
    id: int
    account_id: str
    folder: str
    uid: int
    message_id: str
    subject: str
    sender: str
    recipients: str
    date_sent: Optional[str] = None
    date_received: Optional[str] = None
    size_bytes: int = 0
    flags: str = ""
    has_attachments: bool = False
    body_preview: str = ""
    sha256: str = ""


class EmailDetail(EmailSummary):
    cc: str = ""
    bcc: str = ""
    reply_to: str = ""
    body_text: str = ""
    body_html: str = ""
    headers: dict = Field(default_factory=dict)
    attachments: List[AttachmentInfo] = Field(default_factory=list)
    eml_relative_path: str = ""


class SearchQuery(BaseModel):
    query: str = ""
    account_id: Optional[str] = None
    folder: Optional[str] = None
    sender: Optional[str] = None
    recipient: Optional[str] = None
    subject: Optional[str] = None
    has_attachment: Optional[bool] = None
    date_from: Optional[str] = None
    date_to: Optional[str] = None
    limit: int = 50
    offset: int = 0


class ExportImapRequest(BaseModel):
    profile_id: Optional[str] = None
    folder: Optional[str] = None  # None exporta todas
    target_host: str
    target_port: int = 993
    target_ssl_mode: str = "SSL"  # "SSL", "STARTTLS", "PLAIN"
    target_username: str
    target_password: str
    target_folder_prefix: str = ""
    preserve_internal_date: bool = True
    preserve_flags: bool = True


class ExportPstRequest(BaseModel):
    profile_id: Optional[str] = None
    folder: Optional[str] = None
    output_path: str


class ExportEmlRequest(BaseModel):
    profile_id: Optional[str] = None
    folder: Optional[str] = None
    output_dir: str


class ExportMboxRequest(BaseModel):
    profile_id: Optional[str] = None
    folder: Optional[str] = None
    output_path: str


class TestConnectionRequest(BaseModel):
    host: str
    port: int = 993
    ssl_mode: str = "SSL"
    username: str
    password: str
    timeout: int = 30
