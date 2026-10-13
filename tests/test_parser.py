import email
import email.mime.multipart
import email.mime.text
import email.mime.base
import tempfile
import unittest
from pathlib import Path

from mailstore.parser import (
    ParsedEmail,
    clean_html_to_text,
    decode_mime_words,
    parse_email_date,
)


class TestEmailParser(unittest.TestCase):
    def test_decode_mime_words(self):
        # UTF-8 Q-encoded e B-encoded
        encoded = "=?utf-8?B?VGVzdGUgZGUgQWNlbnR1YcOnw6Nv?="
        decoded = decode_mime_words(encoded)
        self.assertEqual(decoded, "Teste de Acentuação")

        plain = "Assunto Normal Sem Acentos"
        self.assertEqual(decode_mime_words(plain), plain)

    def test_parse_email_date(self):
        date_hdr = "Tue, 13 Oct 2026 10:00:00 -0300"
        iso, ts = parse_email_date(date_hdr)
        self.assertIsNotNone(iso)
        self.assertTrue(iso.startswith("2026-10-13"))
        self.assertIsNotNone(ts)

    def test_clean_html_to_text(self):
        html = "<html><body><h1>Olá Mundo</h1><p>Este é um <b>parágrafo</b>.<br>Segunda linha.</p></body></html>"
        txt = clean_html_to_text(html)
        self.assertIn("Olá Mundo", txt)
        self.assertIn("parágrafo", txt)
        self.assertIn("Segunda linha", txt)

    def test_parsed_email_with_attachments(self):
        msg = email.mime.multipart.MIMEMultipart()
        msg["Subject"] = "=?utf-8?B?UmVsYXTDs3JpbyBNZW5zYWw=?="
        msg["From"] = "remetente@empresa.com"
        msg["To"] = "destinatario@cliente.com"
        msg["Cc"] = "copia@empresa.com"
        msg["Date"] = "Tue, 13 Oct 2026 14:30:00 -0300"
        msg["Message-ID"] = "<test-12345@empresa.com>"

        # Corpo texto e HTML
        body_part = email.mime.multipart.MIMEMultipart("alternative")
        body_part.attach(email.mime.text.MIMEText("Texto simples de teste.", "plain", "utf-8"))
        body_part.attach(email.mime.text.MIMEText("<p>Texto <b>HTML</b> de teste.</p>", "html", "utf-8"))
        msg.attach(body_part)

        # Anexo
        att = email.mime.base.MIMEBase("application", "pdf")
        att.set_payload(b"%PDF-1.4 Fake PDF Content")
        att.add_header("Content-Disposition", "attachment", filename="relatorio.pdf")
        msg.attach(att)

        raw_bytes = msg.as_bytes()

        parsed = ParsedEmail(raw_bytes, "acc_1", "INBOX", 101, flags="\\Seen")

        self.assertEqual(parsed.subject, "Relatório Mensal")
        self.assertEqual(parsed.sender, "remetente@empresa.com")
        self.assertEqual(parsed.recipients, "destinatario@cliente.com")
        self.assertEqual(parsed.cc, "copia@empresa.com")
        self.assertEqual(parsed.message_id, "test-12345@empresa.com")
        self.assertEqual(parsed.flags, "\\Seen")
        self.assertEqual(len(parsed.attachments), 1)
        self.assertEqual(parsed.attachments[0]["filename"], "relatorio.pdf")
        self.assertEqual(parsed.attachments[0]["size_bytes"], len(b"%PDF-1.4 Fake PDF Content"))
        self.assertIn("Texto simples de teste", parsed.body_text)
        self.assertIn("HTML", parsed.body_html)

        # Salvar EML
        with tempfile.TemporaryDirectory() as tmpdir:
            tmppath = Path(tmpdir)
            rel_path = parsed.save_eml(tmppath)
            full_saved = tmppath / rel_path
            self.assertTrue(full_saved.exists())
            self.assertEqual(full_saved.read_bytes(), raw_bytes)


if __name__ == "__main__":
    unittest.main()
