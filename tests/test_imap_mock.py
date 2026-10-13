import email.mime.text
import socket
import tempfile
import threading
import time
import unittest
from pathlib import Path

from mailstore.database import Database
from mailstore.export_engine import ExportEngine
from mailstore.imap_engine import (
    BackupProgress,
    BackupRunner,
    test_imap_connection,
)
from mailstore.models import (
    BackupProfile,
    ExportImapRequest,
    ImapConfig,
)


class MockImapServer:
    """Servidor Mock IMAP4 RFC 3501 para testes locais e automatizados."""
    def __init__(self, host="127.0.0.1", port=0):
        self.host = host
        self.server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.server_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server_sock.bind((host, port))
        self.port = self.server_sock.getsockname()[1]
        self.running = False
        self.thread = None
        self.appended_messages = []

    def start(self):
        self.running = True
        self.server_sock.listen(5)
        self.thread = threading.Thread(target=self._accept_loop, daemon=True)
        self.thread.start()

    def stop(self):
        self.running = False
        try:
            self.server_sock.close()
        except Exception:
            pass

    def _accept_loop(self):
        while self.running:
            try:
                client, _ = self.server_sock.accept()
                threading.Thread(target=self._handle_client, args=(client,), daemon=True).start()
            except Exception:
                break

    def _handle_client(self, client):
        client_file = client.makefile("rwb", buffering=0)
        client_file.write(b"* OK Mock IMAP Server Ready\r\n")

        selected_folder = None

        while self.running:
            line = client_file.readline()
            if not line:
                break
            line_str = line.decode("latin1", errors="replace").strip()
            if not line_str:
                continue

            parts = line_str.split(" ", 2)
            tag = parts[0]
            cmd = parts[1].upper() if len(parts) > 1 else ""
            args = parts[2] if len(parts) > 2 else ""

            if cmd == "CAPABILITY":
                client_file.write(b"* CAPABILITY IMAP4rev1\r\n")
                client_file.write(f"{tag} OK CAPABILITY completed\r\n".encode())

            elif cmd == "LOGIN":
                client_file.write(f"{tag} OK [CAPABILITY IMAP4rev1] LOGIN completed\r\n".encode())

            elif cmd == "LIST":
                client_file.write(b'* LIST (\\HasNoChildren) "/" "INBOX"\r\n')
                client_file.write(b'* LIST (\\HasNoChildren) "/" "Enviados"\r\n')
                client_file.write(f"{tag} OK LIST completed\r\n".encode())

            elif cmd == "SELECT":
                selected_folder = args.replace('"', '')
                client_file.write(b"* 2 EXISTS\r\n")
                client_file.write(b"* 0 RECENT\r\n")
                client_file.write(b"* OK [UIDVALIDITY 12345] UIDs valid\r\n")
                client_file.write(f"{tag} OK [READ-ONLY] SELECT completed\r\n".encode())

            elif cmd == "UID":
                uid_sub = args.split(" ", 1)
                sub_cmd = uid_sub[0].upper()
                sub_args = uid_sub[1] if len(uid_sub) > 1 else ""

                if sub_cmd == "SEARCH":
                    client_file.write(b"* SEARCH 101 102\r\n")
                    client_file.write(f"{tag} OK UID SEARCH completed\r\n".encode())

                elif sub_cmd == "FETCH":
                    # Simula resposta de FETCH
                    fetch_uid = sub_args.split()[0]
                    sample_email = (
                        f"From: remetente@exemplo.com\r\n"
                        f"To: destino@exemplo.com\r\n"
                        f"Subject: Mensagem de Teste UID {fetch_uid}\r\n"
                        f"Date: Tue, 13 Oct 2026 15:00:00 -0300\r\n"
                        f"Message-ID: <test-{fetch_uid}@exemplo.com>\r\n\r\n"
                        f"Corpo do e-mail de teste para o UID {fetch_uid}."
                    ).encode("utf-8")

                    resp_header = f'* {fetch_uid} FETCH (UID {fetch_uid} FLAGS (\\Seen) INTERNALDATE "13-Oct-2026 15:00:00 -0300" RFC822 {{{len(sample_email)}}}\r\n'.encode()
                    client_file.write(resp_header)
                    client_file.write(sample_email)
                    client_file.write(b")\r\n")
                    client_file.write(f"{tag} OK UID FETCH completed\r\n".encode())

            elif cmd == "CREATE":
                client_file.write(f"{tag} OK CREATE completed\r\n".encode())

            elif cmd == "APPEND":
                # Lê bytes literais do append se houver
                if "{" in line_str and line_str.endswith("}"):
                    size_str = line_str[line_str.rfind("{") + 1 : -1]
                    try:
                        msg_size = int(size_str)
                        client_file.write(b"+ Ready for literal data\r\n")
                        data_read = client_file.read(msg_size)
                        self.appended_messages.append(data_read)
                    except Exception:
                        pass
                client_file.write(f"{tag} OK [APPENDUID 12345 999] APPEND completed\r\n".encode())

            elif cmd == "LOGOUT":
                client_file.write(b"* BYE Mock IMAP logging out\r\n")
                client_file.write(f"{tag} OK LOGOUT completed\r\n".encode())
                break

            else:
                client_file.write(f"{tag} OK {cmd} completed\r\n".encode())

        client.close()


class TestImapMockIntegration(unittest.TestCase):
    def setUp(self):
        self.server = MockImapServer()
        self.server.start()

        self.tmpdir = tempfile.TemporaryDirectory()
        self.tmp_path = Path(self.tmpdir.name)
        self.db = Database(self.tmp_path / "mock_test.db")

        # Configura STORAGE_DIR
        import mailstore.imap_engine
        import mailstore.export_engine
        self.storage_dir = self.tmp_path / "eml_storage"
        self.storage_dir.mkdir(parents=True, exist_ok=True)
        self.orig_storage = mailstore.imap_engine.STORAGE_DIR
        mailstore.imap_engine.STORAGE_DIR = self.storage_dir
        mailstore.export_engine.STORAGE_DIR = self.storage_dir

    def tearDown(self):
        import mailstore.imap_engine
        import mailstore.export_engine
        mailstore.imap_engine.STORAGE_DIR = self.orig_storage
        mailstore.export_engine.STORAGE_DIR = self.orig_storage
        self.server.stop()
        self.tmpdir.cleanup()

    def test_connection_and_discovery(self):
        cfg = ImapConfig(
            host="127.0.0.1",
            port=self.server.port,
            ssl_mode="PLAIN",
            username="user@mock.com",
            password="mockpassword"
        )
        res = test_imap_connection(cfg)
        self.assertTrue(res["success"])
        self.assertEqual(res["folders_count"], 2)
        self.assertIn("INBOX", res["folders"])

    def test_backup_runner_end_to_end(self):
        cfg = ImapConfig(
            host="127.0.0.1",
            port=self.server.port,
            ssl_mode="PLAIN",
            username="user@mock.com",
            password="mockpassword"
        )
        profile = BackupProfile(
            id="mock_acc",
            name="Mock Backup",
            imap=cfg,
            selected_folders=["INBOX"]
        )
        self.db.save_profile(profile)

        runner = BackupRunner(self.db)
        prog = BackupProgress("mock_acc")
        success = runner.run_backup(profile, prog)

        self.assertTrue(success)
        self.assertEqual(prog.synced_count, 2)

        # Verifica banco de dados
        messages, total = self.db.list_messages("mock_acc")
        self.assertEqual(total, 2)
        self.assertEqual(messages[0].account_id, "mock_acc")

        # Verifica que arquivos .eml foram salvos
        eml_files = list(self.storage_dir.rglob("*.eml"))
        self.assertEqual(len(eml_files), 2)

    def test_export_to_imap_migration(self):
        # Primeiro insere um e-mail local
        eml_dir = self.storage_dir / "acc_exp" / "INBOX"
        eml_dir.mkdir(parents=True, exist_ok=True)
        test_file = eml_dir / "99_test.eml"
        test_file.write_bytes(b"From: migracao@teste.com\r\nSubject: Migracao Ok\r\n\r\nOla destino!")

        self.db.upsert_folder("acc_exp", "INBOX")
        self.db.insert_message(
            account_id="acc_exp",
            folder="INBOX",
            uid=99,
            message_id="msg-mig",
            subject="Migracao Ok",
            sender="migracao@teste.com",
            recipients="alvo@teste.com",
            cc="",
            bcc="",
            reply_to="",
            date_sent="2026-10-13T12:00:00",
            date_received="2026-10-13T12:00:00",
            size_bytes=test_file.stat().st_size,
            flags="\\Seen",
            has_attachments=False,
            body_preview="Ola destino!",
            body_text_full="Ola destino!",
            sha256="hash123",
            eml_relative_path="acc_exp/INBOX/99_test.eml",
            attachments=[]
        )

        req = ExportImapRequest(
            profile_id="acc_exp",
            target_host="127.0.0.1",
            target_port=self.server.port,
            target_ssl_mode="PLAIN",
            target_username="user@mock.com",
            target_password="mockpassword",
            target_folder_prefix="Restaurado"
        )

        engine = ExportEngine(self.db)
        ok = engine.export_to_imap(req)
        self.assertTrue(ok)
        self.assertEqual(len(self.server.appended_messages), 1)
        self.assertIn(b"Migracao Ok", self.server.appended_messages[0])


if __name__ == "__main__":
    unittest.main()
