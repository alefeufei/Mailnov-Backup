"""Parser e extrator robusto de mensagens RFC 822 / MIME (.eml)."""

import email
import email.policy
import email.utils
import hashlib
import re
from datetime import datetime
from email.header import decode_header
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


def decode_mime_words(header_value: Optional[str]) -> str:
    """Decodifica palavras RFC 2047 codificadas em cabeçalhos (ex: =?utf-8?B?...?=)."""
    if not header_value:
        return ""
    try:
        decoded_fragments = decode_header(header_value)
        result = []
        for fragment, encoding in decoded_fragments:
            if isinstance(fragment, bytes):
                encoding = encoding or "utf-8"
                try:
                    result.append(fragment.decode(encoding, errors="replace"))
                except LookupError:
                    result.append(fragment.decode("utf-8", errors="replace"))
            else:
                result.append(str(fragment))
        return "".join(result).strip()
    except Exception:
        return str(header_value)


def parse_email_date(date_header: Optional[str]) -> Tuple[Optional[str], Optional[float]]:
    """Converte o cabeçalho Date para ISO 8601 e timestamp Unix."""
    if not date_header:
        return None, None
    try:
        parsed_tuple = email.utils.parsedate_to_datetime(date_header)
        if parsed_tuple:
            iso = parsed_tuple.isoformat()
            ts = parsed_tuple.timestamp()
            return iso, ts
    except Exception:
        pass
    return None, None


def clean_html_to_text(html_content: str) -> str:
    """Converte conteúdo HTML em texto limpo para busca e pré-visualização."""
    if not html_content:
        return ""
    # Remove scripts e styles
    text = re.sub(r"<(script|style).*?>.*?</\1>", " ", html_content, flags=re.DOTALL | re.IGNORECASE)
    # Quebra tags de bloco em quebras de linha
    text = re.sub(r"<(br|p|div|tr|li)[^>]*>", "\n", text, flags=re.IGNORECASE)
    # Remove todas as outras tags
    text = re.sub(r"<[^>]+>", " ", text)
    # Converte entidades HTML comuns
    text = text.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", '"')
    # Normaliza espaços múltiplos
    lines = [re.sub(r"[ \t]+", " ", line).strip() for line in text.splitlines()]
    return "\n".join(l for l in lines if l).strip()


class ParsedEmail:
    def __init__(
        self,
        raw_bytes: bytes,
        account_id: str,
        folder: str,
        uid: int,
        flags: str = "",
        internal_date: Optional[str] = None
    ):
        self.raw_bytes = raw_bytes
        self.account_id = account_id
        self.folder = folder
        self.uid = uid
        self.flags = flags
        self.size_bytes = len(raw_bytes)
        self.sha256 = hashlib.sha256(raw_bytes).hexdigest()

        # Parse usando standard library email com policy moderna
        self.msg = email.message_from_bytes(raw_bytes, policy=email.policy.default)

        # Cabeçalhos principais
        self.subject = decode_mime_words(self.msg.get("subject", "")) or "(Sem assunto)"
        self.sender = decode_mime_words(self.msg.get("from", "")) or "Desconhecido"
        self.recipients = decode_mime_words(self.msg.get("to", ""))
        self.cc = decode_mime_words(self.msg.get("cc", ""))
        self.bcc = decode_mime_words(self.msg.get("bcc", ""))
        self.reply_to = decode_mime_words(self.msg.get("reply-to", ""))
        self.message_id = (self.msg.get("message-id", "") or f"gen-{self.sha256[:16]}").strip("<> ")

        # Data
        date_hdr = self.msg.get("date")
        iso_sent, _ = parse_email_date(date_hdr)
        self.date_sent = iso_sent or internal_date or datetime.now().isoformat()
        self.date_received = internal_date or self.date_sent

        # Extração de corpo e anexos
        self.body_text = ""
        self.body_html = ""
        self.attachments: List[Dict[str, Any]] = []
        self._extract_parts()

        # Prévia de texto
        if self.body_text:
            plain_sample = self.body_text
        elif self.body_html:
            plain_sample = clean_html_to_text(self.body_html)
        else:
            plain_sample = ""
        
        self.body_preview = " ".join(plain_sample.split())[:280]

    def _extract_parts(self):
        """Varre as partes MIME para isolar texto puro, HTML e anexos."""
        part_idx = 0
        for part in self.msg.walk():
            # Pula contêineres multipart principais
            if part.is_multipart():
                continue

            content_type = part.get_content_type().lower()
            content_disposition = str(part.get_content_disposition() or "").lower()
            filename = part.get_filename()

            if filename:
                filename = decode_mime_words(filename)

            is_attachment = (
                content_disposition == "attachment"
                or (filename is not None and content_disposition != "inline")
            )

            # Se for texto e não for explicitamente anexo
            if not is_attachment:
                if content_type == "text/plain" and not self.body_text:
                    try:
                        self.body_text = part.get_content()
                    except Exception:
                        payload = part.get_payload(decode=True)
                        if payload:
                            self.body_text = payload.decode("utf-8", errors="replace")
                    continue
                elif content_type == "text/html" and not self.body_html:
                    try:
                        self.body_html = part.get_content()
                    except Exception:
                        payload = part.get_payload(decode=True)
                        if payload:
                            self.body_html = payload.decode("utf-8", errors="replace")
                    continue

            # Se chegou aqui ou tem nome de arquivo, trata como anexo
            if filename or is_attachment:
                part_idx += 1
                att_payload = part.get_payload(decode=True)
                att_size = len(att_payload) if att_payload else 0
                clean_name = filename or f"anexo_{part_idx}.dat"
                self.attachments.append({
                    "filename": clean_name,
                    "content_type": content_type,
                    "size_bytes": att_size,
                    "part_index": part_idx,
                    "payload": att_payload
                })

    def save_eml(self, base_storage_dir: Path) -> str:
        """Salva os bytes do EML em disco e retorna o caminho relativo."""
        # Organização por conta e pasta (sanitizada)
        folder_safe = re.sub(r'[\/:*?"<>|]', '_', self.folder).strip(". ") or "Root"
        account_safe = re.sub(r'[\/:*?"<>|]', '_', self.account_id).strip(". ")
        
        target_dir = base_storage_dir / account_safe / folder_safe
        target_dir.mkdir(parents=True, exist_ok=True)

        filename = f"{self.uid}_{self.sha256[:12]}.eml"
        target_file = target_dir / filename

        # Escrita atômica segura
        tmp_file = target_file.with_suffix(".tmp")
        tmp_file.write_bytes(self.raw_bytes)
        tmp_file.replace(target_file)

        return str(target_file.relative_to(base_storage_dir))
