"""Interface de Linha de Comando (CLI) do MailStore Backup."""

import argparse
import sys
import webbrowser
from datetime import datetime
from pathlib import Path

from mailstore.config import BASE_DIR, STORAGE_DIR
from mailstore.database import Database
from mailstore.export_engine import ExportEngine, ExportProgress
from mailstore.imap_engine import BackupProgress, BackupRunner
from mailstore.models import (
    BackupProfile,
    ExportEmlRequest,
    ExportImapRequest,
    ExportMboxRequest,
    ImapConfig,
    SearchQuery,
)
from mailstore.pst_engine import PstExportProgress, PstExporter, is_outlook_com_available
from mailstore.verifier import ArchiveVerifier


def main():
    parser = argparse.ArgumentParser(
        description="Mailnov Backup CLI - Backup de e-mails IMAP e exportação para IMAP/PST/EML/MBOX"
    )
    subparsers = parser.add_subparsers(dest="command", help="Comandos disponíveis")

    # 1. server
    p_server = subparsers.add_parser("server", help="Inicia o servidor web da aplicação")
    p_server.add_argument("--host", default="127.0.0.1", help="Host de escuta (padrão: 127.0.0.1)")
    p_server.add_argument("--port", type=int, default=8000, help="Porta HTTP (padrão: 8000)")
    p_server.add_argument("--open-browser", action="store_true", help="Abre o navegador padrão automaticamente")

    # 2. profiles
    subparsers.add_parser("profiles", help="Lista todos os perfis de backup configurados")

    # 3. add-profile
    p_add = subparsers.add_parser("add-profile", help="Cadastra um novo perfil de backup IMAP")
    p_add.add_argument("--name", required=True, help="Nome do perfil")
    p_add.add_argument("--host", required=True, help="Servidor IMAP (ex: imap.gmail.com)")
    p_add.add_argument("--port", type=int, default=993, help="Porta IMAP (padrão: 993)")
    p_add.add_argument("--ssl", default="SSL", choices=["SSL", "STARTTLS", "PLAIN"], help="Modo SSL")
    p_add.add_argument("--user", required=True, help="E-mail / Usuário")
    p_add.add_argument("--password", required=True, help="Senha ou Senha de Aplicativo")
    p_add.add_argument("--since", default=None, help="Arquivar apenas a partir de (YYYY-MM-DD)")

    # 4. backup
    p_backup = subparsers.add_parser("backup", help="Executa o arquivamento incremental de um perfil")
    p_backup.add_argument("--profile", required=True, help="ID ou Nome do perfil de backup")

    # 5. export-imap
    p_exp_imap = subparsers.add_parser("export-imap", help="Exporta/migra e-mails para outro servidor IMAP")
    p_exp_imap.add_argument("--profile", default=None, help="ID do perfil de origem (opcional)")
    p_exp_imap.add_argument("--folder", default=None, help="Pasta de origem (opcional)")
    p_exp_imap.add_argument("--target-host", required=True, help="Servidor IMAP de destino")
    p_exp_imap.add_argument("--target-port", type=int, default=993, help="Porta IMAP de destino")
    p_exp_imap.add_argument("--target-ssl", default="SSL", choices=["SSL", "STARTTLS", "PLAIN"], help="Modo SSL destino")
    p_exp_imap.add_argument("--target-user", required=True, help="Usuário/E-mail de destino")
    p_exp_imap.add_argument("--target-pass", required=True, help="Senha de destino")
    p_exp_imap.add_argument("--prefix", default="", help="Prefixo da pasta no destino (ex: Restaurado)")

    # 6. export-pst
    p_exp_pst = subparsers.add_parser("export-pst", help="Exporta mensagens para arquivo PST do Microsoft Outlook")
    p_exp_pst.add_argument("--output", required=True, help="Caminho do arquivo de saída .pst")
    p_exp_pst.add_argument("--profile", default=None, help="ID do perfil de origem (opcional)")
    p_exp_pst.add_argument("--folder", default=None, help="Pasta específica a exportar (opcional)")

    # 7. export-eml
    p_exp_eml = subparsers.add_parser("export-eml", help="Exporta mensagens para pasta de arquivos .eml")
    p_exp_eml.add_argument("--output-dir", required=True, help="Diretório de saída")
    p_exp_eml.add_argument("--profile", default=None, help="ID do perfil de origem (opcional)")
    p_exp_eml.add_argument("--folder", default=None, help="Pasta específica a exportar (opcional)")

    # 8. export-mbox
    p_exp_mbox = subparsers.add_parser("export-mbox", help="Exporta mensagens para arquivo .mbox")
    p_exp_mbox.add_argument("--output", required=True, help="Caminho do arquivo .mbox de saída")
    p_exp_mbox.add_argument("--profile", default=None, help="ID do perfil de origem (opcional)")
    p_exp_mbox.add_argument("--folder", default=None, help="Pasta específica a exportar (opcional)")

    # 9. search
    p_search = subparsers.add_parser("search", help="Pesquisa em todo o arquivo local")
    p_search.add_argument("query", help="Termo de busca em texto completo")
    p_search.add_argument("--limit", type=int, default=20, help="Quantidade máxima de resultados")

    # 10. verify
    p_verify = subparsers.add_parser("verify", help="Auditoria de integridade SHA-256 do arquivo")
    p_verify.add_argument("--profile", default=None, help="Filtrar por perfil (opcional)")

    args = parser.parse_args()
    if not args.command:
        parser.print_help()
        sys.exit(1)

    db = Database()

    # --- Execução dos Comandos ---

    if args.command == "server":
        import uvicorn
        url = f"http://{args.host}:{args.port}"
        print(f"=== Mailnov Backup ===")
        print(f"Servidor iniciado em: {url}")
        print(f"Diretório de dados: {BASE_DIR}")
        if args.open_browser:
            webbrowser.open(url)
        uvicorn.run("mailstore.server:app", host=args.host, port=args.port, reload=False)

    elif args.command == "profiles":
        profs = db.list_profiles()
        if not profs:
            print("Nenhum perfil cadastrado. Use 'add-profile' ou a interface web.")
            return
        print(f"{'ID':<10} {'NOME':<25} {'USUÁRIO':<30} {'MSGS':<8} {'STATUS':<12} {'ÚLTIMO ARQUIVAMENTO'}")
        print("-" * 105)
        for p in profs:
            print(f"{p.id:<10} {p.name[:24]:<25} {p.imap.username[:29]:<30} {p.message_count:<8} {p.last_status:<12} {p.last_run or 'Nunca'}")

    elif args.command == "add-profile":
        import uuid
        prof_id = str(uuid.uuid4())[:8]
        new_prof = BackupProfile(
            id=prof_id,
            name=args.name,
            imap=ImapConfig(
                host=args.host,
                port=args.port,
                ssl_mode=args.ssl,
                username=args.user,
                password=args.password
            ),
            since_date=args.since
        )
        db.save_profile(new_prof)
        print(f"Perfil '{args.name}' criado com sucesso! ID: {prof_id}")

    elif args.command == "backup":
        # Localiza por ID ou Nome
        profs = db.list_profiles()
        target = None
        for p in profs:
            if p.id.lower() == args.profile.lower() or p.name.lower() == args.profile.lower():
                target = p
                break
        if not target:
            print(f"Erro: Perfil '{args.profile}' não encontrado.")
            sys.exit(1)

        print(f"Iniciando arquivamento para '{target.name}' ({target.imap.username} em {target.imap.host})...")
        progress = BackupProgress(target.id)

        def log_cb(d):
            if d.get("logs"):
                print(d["logs"][-1])

        runner = BackupRunner(db)
        success = runner.run_backup(target, progress, on_progress=log_cb)
        if success:
            print(f"Arquivamento concluído com sucesso! Total sincronizado: {progress.synced_count} mensagens.")
        else:
            print(f"Arquivamento finalizado com status: {progress.status}")

    elif args.command == "export-imap":
        req = ExportImapRequest(
            profile_id=args.profile,
            folder=args.folder,
            target_host=args.target_host,
            target_port=args.target_port,
            target_ssl_mode=args.target_ssl,
            target_username=args.target_user,
            target_password=args.target_pass,
            target_folder_prefix=args.prefix
        )
        print(f"Iniciando exportação IMAP para {args.target_user}@{args.target_host}...")
        exp = ExportEngine(db)
        progress = ExportProgress("export_imap", 0, args.target_host)

        def log_cb(d):
            if d.get("logs"):
                print(d["logs"][-1])

        ok = exp.export_to_imap(req, progress, on_progress=log_cb)
        print(f"Migração finalizada: {'Sucesso' if ok else 'Falha'}.")

    elif args.command == "export-pst":
        pst_ok, pst_msg = is_outlook_com_available()
        if not pst_ok:
            print(f"Erro: Automação PST do Outlook indisponível: {pst_msg}")
            sys.exit(1)

        print(f"Iniciando exportação PST para: {args.output}...")
        pst_exp = PstExporter(db)
        prog = PstExportProgress(0, args.output)

        def log_cb(d):
            if d.get("logs"):
                print(d["logs"][-1])

        ok = pst_exp.export_to_pst(
            output_pst_path=args.output,
            account_id=args.profile,
            folder_filter=args.folder,
            progress=prog,
            on_progress=log_cb
        )
        print(f"Exportação PST finalizada: {'Sucesso' if ok else 'Falha'}.")

    elif args.command == "export-eml":
        req = ExportEmlRequest(profile_id=args.profile, folder=args.folder, output_dir=args.output_dir)
        print(f"Exportando EMLs para: {args.output_dir}...")
        exp = ExportEngine(db)
        prog = ExportProgress("export_eml", 0, args.output_dir)
        ok = exp.export_to_eml_directory(req, prog)
        print(f"Concluído! {prog.success_count} mensagens exportadas.")

    elif args.command == "export-mbox":
        req = ExportMboxRequest(profile_id=args.profile, folder=args.folder, output_path=args.output)
        print(f"Exportando MBOX para: {args.output}...")
        exp = ExportEngine(db)
        prog = ExportProgress("export_mbox", 0, args.output)
        ok = exp.export_to_mbox(req, prog)
        print(f"Concluído! {prog.success_count} mensagens exportadas para MBOX.")

    elif args.command == "search":
        q = SearchQuery(query=args.query, limit=args.limit)
        items, total = db.search_messages(q)
        print(f"Resultados da pesquisa por '{args.query}': {total} encontrados.")
        print("-" * 90)
        for m in items:
            date_str = m.date_sent[:10] if m.date_sent else "----/--/--"
            print(f"[{m.id}] {date_str} | De: {m.sender[:25]:<25} | Assunto: {m.subject[:40]}")

    elif args.command == "verify":
        print("Iniciando auditoria de integridade criptográfica SHA-256...")
        v = ArchiveVerifier(db)
        rep = v.verify_archive(args.profile)
        print(f"Verificação concluída!")
        print(f"Total verificado: {rep.total_checked}")
        print(f"Íntegros:        {rep.valid_count}")
        print(f"Ausentes:        {rep.missing_count}")
        print(f"Corrompidos:     {rep.corrupt_count}")


if __name__ == "__main__":
    main()
