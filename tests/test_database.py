import tempfile
import unittest
from pathlib import Path

from mailstore.database import Database
from mailstore.models import BackupProfile, ImapConfig, SearchQuery


class TestDatabase(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory()
        self.db_path = Path(self.tmpdir.name) / "test_mailstore.db"
        self.db = Database(self.db_path)

    def tearDown(self):
        self.tmpdir.cleanup()

    def test_profile_crud(self):
        profile = BackupProfile(
            id="p1",
            name="Gmail Teste",
            imap=ImapConfig(
                host="imap.gmail.com",
                port=993,
                ssl_mode="SSL",
                username="test@gmail.com",
                password="secret_password"
            ),
            selected_folders=["INBOX", "Sent"],
            since_date="2026-01-01"
        )
        self.db.save_profile(profile)

        # Buscar
        fetched = self.db.get_profile("p1")
        self.assertIsNotNone(fetched)
        self.assertEqual(fetched.name, "Gmail Teste")
        self.assertEqual(fetched.imap.username, "test@gmail.com")
        self.assertEqual(fetched.selected_folders, ["INBOX", "Sent"])

        # Listar
        all_p = self.db.list_profiles()
        self.assertEqual(len(all_p), 1)

        # Excluir
        self.db.delete_profile("p1")
        self.assertIsNone(self.db.get_profile("p1"))

    def test_message_insert_and_search(self):
        # Inserir pasta
        self.db.upsert_folder("acc1", "INBOX")

        # Inserir mensagens
        msg_id = self.db.insert_message(
            account_id="acc1",
            folder="INBOX",
            uid=101,
            message_id="msg-101",
            subject="Fatura Provedor de Internet",
            sender="financeiro@provedor.com",
            recipients="cliente@dominio.com",
            cc="",
            bcc="",
            reply_to="",
            date_sent="2026-10-10T10:00:00",
            date_received="2026-10-10T10:00:00",
            size_bytes=5432,
            flags="\\Seen",
            has_attachments=True,
            body_preview="Sua fatura do mês está pronta para pagamento.",
            body_text_full="Sua fatura do mês está pronta para pagamento no valor de R$ 150,00.",
            sha256="abc123sha256",
            eml_relative_path="acc1/INBOX/101_abc123.eml",
            attachments=[{"filename": "fatura.pdf", "size_bytes": 4000, "content_type": "application/pdf", "part_index": 1}]
        )

        self.assertGreater(msg_id, 0)

        # Detalhe
        detail = self.db.get_message_detail(msg_id)
        self.assertIsNotNone(detail)
        self.assertEqual(detail.subject, "Fatura Provedor de Internet")
        self.assertEqual(len(detail.attachments), 1)
        self.assertEqual(detail.attachments[0].filename, "fatura.pdf")

        # Busca em texto completo
        q = SearchQuery(query="fatura")
        items, total = self.db.search_messages(q)
        self.assertEqual(total, 1)
        self.assertEqual(items[0].id, msg_id)

        # Filtro de anexo
        q_att = SearchQuery(has_attachment=True)
        items_att, total_att = self.db.search_messages(q_att)
        self.assertEqual(total_att, 1)

        # Filtro por pasta
        items_f, total_f = self.db.list_messages("acc1", "INBOX")
        self.assertEqual(total_f, 1)


if __name__ == "__main__":
    unittest.main()
