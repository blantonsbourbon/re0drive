#!/usr/bin/env python3
"""Local stand-in for the RE0 relay API and 115 share endpoints, for offline end-to-end tests.

Verifies the HMAC signature on every relay request with a TEST key and records calls.
Usage: python3 tests/mock_upstream.py [port] [test-key]
Run re0drive with RE0DRIVE_115_BASE_URL=http://127.0.0.1:<port> and settings
re0_base_url=http://127.0.0.1:<port>, secret=<test-key>.
"""
import hashlib, hmac, json, sys, urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9799
KEY = (sys.argv[2] if len(sys.argv) > 2 else "mock-test-key").encode()
CALLS = []
POINTS = {"m_cheap": 2, "m_pricey": 50, "m_owned": 0}

def canonical(method, path, query, inst, ts, nonce, body):
    pairs = sorted((k, v) for k, vs in urllib.parse.parse_qs(query, keep_blank_values=True).items() if k not in ("sig", "signature") for v in vs)
    return "\n".join([method.upper(), path, urllib.parse.urlencode(pairs), inst, ts, nonce, hashlib.sha256(body).hexdigest()])

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, obj, code=200):
        b = json.dumps(obj).encode(); self.send_response(code)
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def handle_any(self, method):
        u = urllib.parse.urlsplit(self.path); body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        if u.path == "/_calls": return self.send(CALLS)
        if u.path.startswith("/share/"):
            CALLS.append({"m": method, "p": u.path, "q": u.query, "cookie": bool(self.headers.get("Cookie")), "body": body.decode()})
            if u.path == "/share/snap":
                return self.send({"state": True, "data": {"count": 2, "list": [{"fid": "111"}, {"cid": "222"}]}})
            if u.path == "/share/receive":
                if not self.headers.get("Cookie"): return self.send({"state": False, "error": "not logged in"})
                return self.send({"state": True})
        h = self.headers
        exp = hmac.new(KEY, canonical(method, u.path, u.query, h.get("X-Install-Id", ""), h.get("X-Timestamp", ""), h.get("X-Nonce", ""), body).encode(), hashlib.sha256).hexdigest()
        ok = hmac.compare_digest(exp, h.get("X-Signature", ""))
        CALLS.append({"m": method, "p": u.path, "sig_ok": ok, "body": body.decode()})
        if not ok: return self.send({"success": False, "code": "401", "message": "bad signature"}, 401)
        ok_ = lambda d: self.send({"success": True, "code": "200", "data": d})
        if u.path == "/api/token/status": return ok_({"has_access_token": True})
        if u.path == "/api/me": return ok_({"nickname": "mock-user", "points": 42, "level": "Lv3"})
        if u.path == "/api/checkin": return ok_({"message": "签到成功（mock）", "points": 1})
        if u.path.startswith("/api/resources/") and method == "GET":
            return ok_({"115": [
                {"slug": "m_cheap", "title": "Mock Movie 2160p REMUX", "unlock_points": 2, "quality": "2160p", "size": "60 GB"},
                {"slug": "m_pricey", "title": "Mock Movie 2160p WEB-DL", "unlock_points": 50, "quality": "2160p"},
                {"slug": "m_owned", "title": "Mock Movie 1080p", "is_unlocked": True, "quality": "1080p"}],
                "quark": [{"slug": "q_x", "title": "not 115"}]})
        if u.path.startswith("/api/shares/"):
            slug = urllib.parse.unquote(u.path.rsplit("/", 1)[1])
            return ok_({"unlock_points": POINTS.get(slug), "is_unlocked": slug == "m_owned"})
        if u.path == "/api/resources/unlock":
            slug = json.loads(body)["slug"]
            return ok_({"full_url": f"https://115.com/s/sw{slug.replace('_','')}", "access_code": "ab12"})
        return self.send({"success": False, "code": "404"}, 404)
    def do_GET(self): self.handle_any("GET")
    def do_POST(self): self.handle_any("POST")

print(f"mock upstream on 127.0.0.1:{PORT}", flush=True)
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
