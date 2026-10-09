#!/usr/bin/env python3
"""Offline import from a local TgtoDrive native client. Run with its matching Python runtime.
No credentials are printed. The server encrypts and removes bootstrap.json on next start.
"""
import argparse
import enum
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("module", type=Path, help="local hdhive_user_client.so (or .pyd)")
parser.add_argument("--output", type=Path, default=Path("data/bootstrap.json"))
parser.add_argument("--install-file", type=Path, help="optional existing re0_install.json to retain authorization")
args = parser.parse_args()
if not args.module.is_file():
    parser.error("client module does not exist")
if args.output.exists():
    parser.error("output exists; import or remove that file first")

def block_network(event, details):
    if event in {"socket.connect", "socket.connect_ex", "socket.getaddrinfo", "subprocess.Popen", "os.system"}:
        raise RuntimeError("Offline importer forbids network and subprocesses")
sys.addaudithook(block_network)

# Only the import-time type dependencies are needed. No HTTP client is created.
requests = types.ModuleType("requests")
requests.__path__ = []
class HTTPAdapter:
    def __init__(self, *a, **kw): pass
class Session:
    def __init__(self): self.headers = {}
    def mount(self, *a, **kw): pass
    def request(self, *a, **kw): raise RuntimeError("Offline importer")
    def close(self): pass
requests.Session = Session
requests.RequestException = RuntimeError
adapters = types.ModuleType("requests.adapters")
adapters.HTTPAdapter = HTTPAdapter
requests.adapters = adapters
urllib3 = types.ModuleType("urllib3")
urllib3.__path__ = []
backend = types.ModuleType("urllib3.backend")
class HttpVersion(enum.Enum): h11 = 11; h2 = 20; h3 = 30
backend.HttpVersion = HttpVersion
urllib3.backend = backend
sys.modules.update({"requests": requests, "requests.adapters": adapters, "urllib3": urllib3, "urllib3.backend": backend})

with tempfile.TemporaryDirectory(prefix="re0drive-import-") as temp:
    os.environ.update({"RE0_INSTALL_FILE": str(Path(temp) / "install.json"), "RE0_USER_INFO_FILE": str(Path(temp) / "user.json"), "RE0_AUTH_STATUS_FILE": str(Path(temp) / "auth.json")})
    spec = importlib.util.spec_from_file_location("hdhive_user_client", args.module.resolve())
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    key = bytes(module.load_shared_secret_bytes()).decode("utf-8")
    if not key:
        raise SystemExit("Client returned an empty signing key")
    payload = {"re0_shared_secret": key, "re0_base_url": module.DEFAULT_SERVER_BASE_URL}
    if args.install_file:
        existing = json.loads(args.install_file.read_text())
        install_id = existing.get("install_id") if isinstance(existing, dict) else None
        if not isinstance(install_id, str) or not install_id:
            raise SystemExit("Install file does not contain an install_id")
        payload["install_id"] = install_id
    args.output.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump(payload, handle)
print("Connection imported. Start RE0Drive to encrypt the configuration; then authorize RE0 in Settings.")
