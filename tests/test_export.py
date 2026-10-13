import mailbox
import tempfile
import unittest
from pathlib import Path

from mailstore.database import Database
from mailstore.export_engine import ExportEngine
from mailstore.models import ExportEmlRequest, ExportMboxRequest
from mailstore.pst_engine import PstExporter, is_outlook_com_available


class TestExportEngine(unittest.TestCase):
    def setUp(self):
        self.tmpdir = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.tmp_path = Path(self.tmpdir.name)
        self.db_path = self.tmp_path / "test.db"
        self.db = Database(self.db_path)

        # Cria pasta de armazenamento fake
        self.storage_dir = self.tmp_path / "storage"
        self.storage_dir.mkdir(parents=True, exist_ok=True)

        # Modifica temporariamente STORAGE_DIR no módulo
        import mailstore.export_engine
        import mailstore.pst_engine
        self.orig_export_storage = mailstore.export_engine.STORAGE_DIR
        self.orig_pst_storage = mailstore.pst_engine.STORAGE_DIR
        mailstore.export_engine.STORAGE_DIR = self.storage_dir
        mailstore.pst_engine.STORAGE_DIR = self.storage_dir

        # Cria EML fake no disco
        eml_folder = self.storage_dir / "acc_test" / "INBOX"
        eml_folder.mkdir(parents=True, exist_ok=True)
        self.eml_file = eml_folder / "1_test.eml"
        self.eml_file.write_bytes(
            b"From: remetente@teste.com\r\nTo: dest@teste.com\r\nSubject: Teste Export\r\nDate: Tue, 13 Oct 2026 12:00:00 -0300\r\n\r\nCorpo do e-mail exportado."
        )

        # Insere mensagem no banco
        self.db.upsert_folder("acc_test", "INBOX")
        self.db.insert_message(
            account_id="acc_test",
            folder="INBOX",
            uid=1,
            message_id="msg-1",
            subject="Teste Export",
            sender="remetente@teste.com",
            recipients="dest@teste.com",
            cc="",
            bcc="",
            reply_to="",
            date_sent="2026-10-13T12:00:00",
            date_received="2026-10-13T12:00:00",
            size_bytes=self.eml_file.stat().st_size,
            flags="\\Seen",
            has_attachments=False,
            body_preview="Corpo do e-mail exportado.",
            body_text_full="Corpo do e-mail exportado.",
            sha256="abc",
            eml_relative_path="acc_test/INBOX/1_test.eml",
            attachments=[]
        )

    def tearDown(self):
        import mailstore.export_engine
        import mailstore.pst_engine
        mailstore.export_engine.STORAGE_DIR = self.orig_export_storage
        mailstore.pst_engine.STORAGE_DIR = self.orig_pst_storage
        self.tmpdir.cleanup()

    def test_export_eml(self):
        out_dir = self.tmp_path / "eml_out"
        engine = ExportEngine(self.db)
        req = ExportEmlRequest(profile_id="acc_test", output_dir=str(out_dir))
        ok = engine.export_to_eml_directory(req)
        self.assertTrue(ok)

        # Verifica arquivo criado
        created_emls = list(out_dir.rglob("*.eml"))
        self.assertEqual(len(created_emls), 1)
        self.assertIn("Teste Export", created_emls[0].name)

    def test_export_mbox(self):
        out_mbox = self.tmp_path / "emails.mbox"
        engine = ExportEngine(self.db)
        req = ExportMboxRequest(profile_id="acc_test", output_path=str(out_mbox))
        ok = engine.export_to_mbox(req)
        self.assertTrue(ok)
        self.assertTrue(out_mbox.exists())

        # Verifica conteúdo com mailbox.mbox padrão
        box = mailbox.mbox(str(out_mbox))
        self.assertEqual(len(box), 1)
        msg = box[0]
        self.assertEqual(msg["subject"], "Teste Export")
        box.close()

    def test_export_pst(self):
        pst_ok, msg = is_outlook_com_available()
        if not pst_ok:
            self.skipTest("Outlook COM não disponível no ambiente atual")

        out_pst = self.tmp_path / "output_test.pst"
        pst_exp = PstExporter(self.db)
        ok = pst_exp.export_to_pst(str(out_pst), account_id="acc_test")
        self.assertTrue(ok)
        self.assertTrue(out_pst.exists())
        self.assertGreater(out_pst.stat().st_size, 1000)


    def test_export_empty_fails_gracefully(self):
        engine = ExportEngine(self.db)
        req = ExportEmlRequest(profile_id="nonexistent_acc", output_dir=str(self.tmp_path / "out"))
        ok = engine.export_to_eml_directory(req)
        self.assertFalse(ok)

    def test_format_internal_date(self):
        from mailstore.export_engine import format_internal_date
        # Raw IMAP INTERNALDATE
        self.assertEqual(format_internal_date("24-Jun-2026 21:45:01 -0300"), '"24-Jun-2026 21:45:01 -0300"')
        # ISO format
        res_iso = format_internal_date("2026-06-24T21:44:58-03:00")
        self.assertIsNotNone(res_iso)
        self.assertTrue(res_iso.startswith('"') and res_iso.endswith('"'))
        # None
        self.assertIsNone(format_internal_date(None))

    def test_resolve_target_folder(self):
        from mailstore.export_engine import resolve_target_folder
        existing = {"inbox": "INBOX", "sent": "Sent", "trash": "Trash"}

        # 1. INBOX sem prefixo
        self.assertEqual(resolve_target_folder("INBOX", "/", None, existing), "INBOX")

        # 2. INBOX com prefixo
        self.assertEqual(resolve_target_folder("INBOX", "/", "Restaurado", existing), "Restaurado/INBOX")

        # 3. Subpasta com delimitador ponto na origem (INBOX.Sent) convertendo para / no destino
        self.assertEqual(resolve_target_folder("INBOX.Sent", "/", None, existing), "Sent")

        # 4. Subpasta com prefixo
        self.assertEqual(resolve_target_folder("INBOX.Sent", "/", "Restaurado", existing), "Restaurado/Sent")

        # 5. Subpasta arbitrária
        self.assertEqual(resolve_target_folder("INBOX.Clientes.2026", "/", "Restaurado", existing), "Restaurado/Clientes/2026")

        # 6. Servidor Dovecot com namespace INBOX. obrigatório
        dovecot_existing = {"inbox": "INBOX", "inbox.sent": "INBOX.Sent", "inbox.trash": "INBOX.Trash"}
        self.assertEqual(resolve_target_folder("INBOX.Sent", ".", None, dovecot_existing, "INBOX."), "INBOX.Sent")
        self.assertEqual(resolve_target_folder("INBOX.Sent", ".", "Restaurado", dovecot_existing, "INBOX."), "INBOX.Restaurado.Sent")
        self.assertEqual(resolve_target_folder("INBOX", ".", "Restaurado", dovecot_existing, "INBOX."), "INBOX.Restaurado.INBOX")
        self.assertEqual(resolve_target_folder("INBOX.Outra", ".", None, dovecot_existing, "INBOX."), "INBOX.Outra")


if __name__ == "__main__":
    unittest.main()

