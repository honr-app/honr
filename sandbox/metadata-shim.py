#!/usr/bin/env python3
"""Minimal GCE metadata server.

google-auth walks its ADC chain and ends at the metadata server. OpenShell
blocks the real one (169.254.169.254) as SSRF hardening, so we serve the
gateway-minted, short-lived Vertex token here instead. No long-lived
credential ever enters the sandbox.
"""
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

TOKEN = os.environ.get("GOOGLE_VERTEX_AI_TOKEN", "")
PROJECT = os.environ.get("GOOGLE_CLOUD_PROJECT") or os.environ.get("ANTHROPIC_VERTEX_PROJECT_ID", "")

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, body, ctype="application/json"):
        b = body.encode()
        self.send_response(200)
        self.send_header("Metadata-Flavor", "Google")
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        p = self.path.split("?")[0].rstrip("/")
        if p.endswith("/token"):
            self._send(json.dumps({"access_token": TOKEN, "expires_in": 3500, "token_type": "Bearer"}))
        elif p.endswith("/service-accounts/default/email"):
            self._send("default", "text/plain")
        elif p.endswith("/project/project-id"):
            self._send(PROJECT, "text/plain")
        elif p.endswith("/universe/universe-domain"):
            self._send("googleapis.com", "text/plain")
        elif p.endswith("/service-accounts/default/scopes"):
            self._send("https://www.googleapis.com/auth/cloud-platform", "text/plain")
        elif "/service-accounts" in p:
            self._send("default/\n", "text/plain")
        else:
            self._send("computeMetadata/\n", "text/plain")

    def log_message(self, *a):
        pass

HTTPServer(("127.0.0.1", 8127), H).serve_forever()
