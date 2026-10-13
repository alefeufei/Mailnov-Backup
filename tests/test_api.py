import unittest
from fastapi.testclient import TestClient
from mailstore.server import app


class TestRestApi(unittest.TestCase):
    def setUp(self):
        self.client = TestClient(app)

    def test_status_endpoint(self):
        response = self.client.get("/api/status")
        self.assertEqual(response.status_code, 200)
        data = response.json()
        self.assertEqual(data["status"], "online")
        self.assertIn("pst_support", data)

    def test_profile_lifecycle(self):
        # 1. Cria perfil
        payload = {
            "name": "Conta Teste API",
            "imap": {
                "host": "imap.exemplo.com",
                "port": 993,
                "ssl_mode": "SSL",
                "username": "api@exemplo.com",
                "password": "senha123"
            },
            "selected_folders": ["INBOX"]
        }
        res_post = self.client.post("/api/profiles", json=payload)
        self.assertEqual(res_post.status_code, 200)
        created = res_post.json()
        prof_id = created["id"]
        self.assertEqual(created["name"], "Conta Teste API")

        # 2. Busca perfil
        res_get = self.client.get(f"/api/profiles/{prof_id}")
        self.assertEqual(res_get.status_code, 200)
        self.assertEqual(res_get.json()["id"], prof_id)

        # 3. Atualiza perfil
        res_put = self.client.put(f"/api/profiles/{prof_id}", json={"name": "Conta Renomeada"})
        self.assertEqual(res_put.status_code, 200)
        self.assertEqual(res_put.json()["name"], "Conta Renomeada")

        # 4. Remove perfil
        res_del = self.client.delete(f"/api/profiles/{prof_id}")
        self.assertEqual(res_del.status_code, 200)

        # 5. Verifica se foi removido
        res_check = self.client.get(f"/api/profiles/{prof_id}")
        self.assertEqual(res_check.status_code, 404)

    def test_search_and_folders_endpoints(self):
        res_folders = self.client.get("/api/folders")
        self.assertEqual(res_folders.status_code, 200)
        self.assertIsInstance(res_folders.json(), list)

        res_search = self.client.post("/api/search", json={"query": "teste", "limit": 10})
        self.assertEqual(res_search.status_code, 200)
        self.assertIn("items", res_search.json())
        self.assertIn("total", res_search.json())

    def test_verify_endpoint(self):
        res_verify = self.client.post("/api/verify")
        self.assertEqual(res_verify.status_code, 200)
        data = res_verify.json()
        self.assertIn("total_checked", data)
        self.assertIn("valid_count", data)


if __name__ == "__main__":
    unittest.main()
