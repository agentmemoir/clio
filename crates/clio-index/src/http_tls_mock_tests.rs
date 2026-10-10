// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for verified-TLS mock testing against an ephemeral
//! HTTPS server using custom root certificates.
//!
//! ## Owns
//! - Verified TLS connection with a trusted custom root CA succeeding and returning JSON
//! - Verified TLS connection failing closed on untrusted or invalid certificates
//! - Ephemeral Python HTTPS test server lifecycle with self-signed TLS credentials
//!
//! ## Does not own
//! - HTTP transport implementation (see `http.rs`)
//! - Embedding sidecar integration tests (see `embed_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to mock TLS verification tests should normally require changes here.
//! Changes to transport internals should not require changes here.
//!
//! Keep this module focused on TLS transport verification testing.

use crate::embed::is_retryable;
use crate::http::{post_json, post_json_with_ca};
use serde_json::json;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static COUNTER: AtomicU64 = AtomicU64::new(0);
const TIMEOUT: Duration = Duration::from_millis(5000);

const PYTHON_SERVER_SCRIPT: &str = r#"
import os, sys, ssl, http.server, subprocess

td = sys.argv[1]
ca_key = os.path.join(td, 'ca_key.pem')
ca_cert = os.path.join(td, 'ca_cert.pem')
srv_key = os.path.join(td, 'srv_key.pem')
srv_csr = os.path.join(td, 'srv_csr.pem')
srv_cert = os.path.join(td, 'srv_cert.pem')

subprocess.run([
    'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
    '-keyout', ca_key, '-out', ca_cert, '-days', '1',
    '-subj', '/CN=ClioTestCA',
    '-addext', 'basicConstraints=critical,CA:TRUE'
], check=True, capture_output=True)

subprocess.run([
    'openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes',
    '-keyout', srv_key, '-out', srv_csr,
    '-subj', '/CN=127.0.0.1',
    '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1'
], check=True, capture_output=True)

subprocess.run([
    'openssl', 'x509', '-req', '-in', srv_csr,
    '-CA', ca_cert, '-CAkey', ca_key, '-CAcreateserial',
    '-out', srv_cert, '-days', '1',
    '-copy_extensions', 'copy'
], check=True, capture_output=True)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get('Content-Length', 0))
        _ = self.rfile.read(length)
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(b'{"embedding": [0.1, 0.2, 0.3], "status": "ok"}')

    def log_message(self, *args):
        pass

server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(srv_cert, srv_key)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
print(server.server_port, flush=True)
server.serve_forever()
"#;

struct MockTlsServer {
    child: Child,
    temp_dir: PathBuf,
    pub ca_pem: Vec<u8>,
    pub url: String,
}

impl MockTlsServer {
    fn spawn() -> Self {
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let unique_name = format!("clio-tls-mock-{pid}-{count}");
        let temp_dir = std::env::temp_dir().join(unique_name);
        std::fs::create_dir_all(&temp_dir).expect("create temp dir for tls mock");

        let mut child = Command::new("python3")
            .arg("-c")
            .arg(PYTHON_SERVER_SCRIPT)
            .arg(&temp_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn python tls mock server");

        let stdout = child.stdout.as_mut().expect("child stdout piped");
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("read port from tls server");
        let port: u16 = line
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("invalid port from mock server `{line}`: {e}"));

        let ca_cert_path = temp_dir.join("ca_cert.pem");
        let ca_pem = std::fs::read(&ca_cert_path).expect("read generated CA cert");
        let url = format!("https://127.0.0.1:{port}/embed");

        Self {
            child,
            temp_dir,
            ca_pem,
            url,
        }
    }
}

impl Drop for MockTlsServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}

#[test]
fn t05_verified_tls_valid_cert_succeeds() {
    let server = MockTlsServer::spawn();
    let body = json!({"inputs": ["agent memory embedding test"]});

    let res = post_json_with_ca(&server.url, &body, TIMEOUT, None, Some(&server.ca_pem))
        .expect("verified TLS request with valid custom CA must succeed");

    assert_eq!(res["status"], "ok");
    assert!(res["embedding"].is_array());
    let arr = res["embedding"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
}

#[test]
fn t06_verified_tls_invalid_cert_fails_closed() {
    let server = MockTlsServer::spawn();
    let body = json!({"inputs": ["agent memory embedding test"]});

    // 1. Without test CA (relying on default WebPKI roots): must fail closed.
    let err_webpki = post_json(&server.url, &body, TIMEOUT, None)
        .expect_err("request without trusted CA must fail closed");
    let msg_webpki = err_webpki.to_string();
    assert!(
        msg_webpki.contains("tls failure"),
        "expected TLS failure on untrusted cert: {msg_webpki}"
    );
    assert!(!is_retryable(&err_webpki), "{msg_webpki}");

    // 2. With an invalid/unrelated CA: must fail closed.
    let unrelated_ca = b"-----BEGIN CERTIFICATE-----\n\
MIIB/zCCAaSgAwIBAgIUeN7h\n\
-----END CERTIFICATE-----\n";
    let err_untrusted = post_json_with_ca(&server.url, &body, TIMEOUT, None, Some(unrelated_ca))
        .expect_err("request with invalid/unrelated CA must fail closed");
    let msg_untrusted = err_untrusted.to_string();
    assert!(
        msg_untrusted.contains("tls failure") || msg_untrusted.contains("tls config"),
        "expected TLS failure on invalid CA: {msg_untrusted}"
    );
    assert!(!is_retryable(&err_untrusted), "{msg_untrusted}");
}
