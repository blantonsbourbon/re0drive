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
AUTHED = set()      # install_ids that completed /auth/start (simulated scan)
QR_POLLS = {}       # 115 qr uid -> poll count
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
        # ---- 115 QR login (qrcodeapi.115.com stand-in): waiting -> scanned -> confirmed ----
        if u.path == "/api/1.0/web/1.0/token/":
            CALLS.append({"m": method, "p": u.path})
            return self.send({"state": 1, "code": 0, "data": {"uid": "mockqr1", "time": 1780000000, "sign": "mocksign", "qrcode": "https://115.com/scan/dg-mockqr1"}})
        if u.path == "/get/status/":
            q = urllib.parse.parse_qs(u.query)
            if q.get("sign") != ["mocksign"]: return self.send({"state": 0, "message": "bad sign"})
            uid = q["uid"][0]; n = QR_POLLS[uid] = QR_POLLS.get(uid, 0) + 1
            CALLS.append({"m": method, "p": u.path, "n": n})
            return self.send({"state": 1, "code": 0, "data": {"status": min(n - 1, 2), "msg": ""}})
        if u.path.startswith("/app/1.0/") and u.path.endswith("/1.0/login/qrcode/"):
            form = urllib.parse.parse_qs(body.decode())
            CALLS.append({"m": method, "p": u.path, "account": form.get("account", [""])[0]})
            if QR_POLLS.get(form.get("account", [""])[0], 0) < 3: return self.send({"state": 0, "message": "not confirmed"})
            return self.send({"state": 1, "data": {"user_id": 7, "user_name": "mock-115", "cookie": {"UID": "7_R2_1", "CID": "mockcid", "SEID": "mockseid", "KID": "mockkid"}}})
        # ---- RE0 relay OAuth entry: a signed GET with query params; visiting it == user finished the scan ----
        if u.path == "/auth/start":
            q = urllib.parse.parse_qs(u.query, keep_blank_values=True)
            g = lambda k: q.get(k, [""])[0]
            exp = hmac.new(KEY, canonical("GET", u.path, u.query, g("install_id"), g("ts"), g("nonce"), b"").encode(), hashlib.sha256).hexdigest()
            ok = hmac.compare_digest(exp, g("sig")); CALLS.append({"m": method, "p": u.path, "sig_ok": ok})
            if ok: AUTHED.add(g("install_id"))
            return self.send({"ok": ok}, 200 if ok else 401)
        if u.path.startswith("/share/"):
            CALLS.append({"m": method, "p": u.path, "q": u.query, "cookie": bool(self.headers.get("Cookie")), "body": body.decode()})
            if u.path == "/share/snap":
                return self.send({"state": True, "data": {"count": 2, "list": [
                    {"fid": "111", "cid": "0", "n": "Mock.Movie.2160p.mkv", "s": "64424509440"},
                    {"cid": "222", "n": "Extras"}]}})
            if u.path == "/share/receive":
                if not self.headers.get("Cookie"): return self.send({"state": False, "error": "not logged in"})
                return self.send({"state": True})
        # NullBR public API shape (https://nullbr.readthedocs.io)
        if (u.path.startswith("/movie/") or u.path.startswith("/tv/")) and u.path.endswith("/115"):
            CALLS.append({"m": method, "p": u.path, "q": u.query, "app": self.headers.get("X-APP-ID"), "key": bool(self.headers.get("X-API-KEY"))})
            if not self.headers.get("X-APP-ID") or not self.headers.get("X-API-KEY"):
                return self.send({"message": "unauthorized"}, 401)
            if u.path.endswith("/999/115"):
                return self.send({"id": 999, "media_type": "movie", "page": 1, "total_page": 1, "items": []})
            return self.send({"id": 1, "media_type": "movie", "page": 1, "total_page": 1, "items": [
                {"title": "NullBR Mock 2160p", "size": "40 GB", "share_link": "https://115.com/s/swnullbr?password=ab12",
                 "resolution": "2160P", "quality": ["REMUX"]}]})
        # Telegram Bot API (token in path)
        if u.path.startswith("/bot"):
            method_name = u.path.rsplit("/", 1)[-1]
            CALLS.append({"m": method, "p": "/bot…/" + method_name})
            body_j = {}
            try: body_j = json.loads(body or b"{}")
            except Exception: pass
            if method_name == "getUpdates":
                return self.send({"ok": True, "result": []})
            if method_name in ("sendMessage", "answerCallbackQuery"):
                return self.send({"ok": True, "result": {"message_id": 1}})
            return self.send({"ok": False, "description": "unknown"}, 400)
        if u.path == "/files":  # 115 folder listing: received items get NEW local ids
            q = urllib.parse.parse_qs(u.query); cid = q.get("cid", [""])[0]
            CALLS.append({"m": method, "p": u.path, "q": u.query, "cookie": bool(self.headers.get("Cookie"))})
            if not self.headers.get("Cookie"): return self.send({"state": False, "error": "not logged in"})
            return self.send({"state": True, "data": [
                {"fid": "9111", "cid": cid, "n": "Mock.Movie.2160p.mkv", "s": 64424509440},
                {"cid": "9222", "pid": cid, "n": "Extras"},
                {"fid": "9333", "cid": cid, "n": "unrelated.nfo", "s": 1}]})
        h = self.headers
        exp = hmac.new(KEY, canonical(method, u.path, u.query, h.get("X-Install-Id", ""), h.get("X-Timestamp", ""), h.get("X-Nonce", ""), body).encode(), hashlib.sha256).hexdigest()
        ok = hmac.compare_digest(exp, h.get("X-Signature", ""))
        CALLS.append({"m": method, "p": u.path, "sig_ok": ok, "body": body.decode()})
        if not ok: return self.send({"success": False, "code": "401", "message": "bad signature"}, 401)
        ok_ = lambda d: self.send({"success": True, "code": "200", "data": d})
        if u.path == "/api/token/status": return ok_({"has_access_token": h.get("X-Install-Id") in AUTHED})
        if u.path == "/api/me": return ok_({"nickname": "mock-user", "points": 42, "level": "Lv3"})
        if u.path == "/api/checkin": return ok_({"message": "签到成功（mock）", "points": 1})
        if u.path == "/api/resources/movie/999":  # relay has nothing: explicit empty list, no fallback
            return self.send({"success": True, "code": "200", "data": [], "total": 0})
        if u.path.startswith("/api/resources/") and method == "GET":
            return ok_({"115": [
                {"slug": "m_cheap", "title": "Mock Movie 2160p REMUX", "unlock_points": 2, "quality": "2160p", "pan_type": "115",
                 "share_size": "60 GB", "video_resolution": ["2160P"], "source": ["REMUX", "Blu-ray"],
                 "subtitle_language": ["简中", "英语"], "subtitle_type": ["内封"], "remark": "杜比视界"},
                {"slug": "m_pricey", "title": "Mock Movie 2160p WEB-DL", "unlock_points": 50, "quality": "2160p"},
                {"slug": "m_owned", "title": "Mock Movie 1080p", "is_unlocked": True, "quality": "1080p"}],
                "quark": [{"slug": "q_x", "title": "not 115", "pan_type": "quark", "video_resolution": ["1080P"], "unlock_points": 0}],
                "ed2k": [{"slug": "e_x", "title": "ed2k link", "pan_type": "ed2k", "source": ["HDTV"], "unlock_points": 1}]})
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
