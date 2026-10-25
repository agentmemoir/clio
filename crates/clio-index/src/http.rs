// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the TLS-capable JSON HTTP transport to
//! configured JSON sidecars (ureq 3 with rustls-verified TLS) and the
//! TEI-compatible client that rides on it.
//!
//! ## Transport Pooling and HTTP/2 Decision
//!
//! Transport calls use a per-call one-shot `ureq` agent by design. Clio is a
//! short-lived CLI process: connection pooling yields no cross-command benefit
//! and risks leaking sockets or carrying stale connections across sidecar
//! restarts, and HTTP/2 adds protocol complexity without a measured win for
//! single-request sidecar calls. Verified TLS stays fail-closed and unchanged
//! (no pooling does not weaken certificate verification). Revisit if a
//! long-running process (server or MCP daemon) starts reusing one sidecar
//! connection for many calls.
//!
//! ## Owns
//! - `post_json` / `post_json_with_ca` / `get_ok` / `get_json` over a per-call
//!   `ureq` agent (verified TLS for `https://`, plain `http://` for local sidecars)
//! - Bounded global timeouts, 64 MiB response cap, endpoint URL parsing that
//!   fails closed on every scheme other than `http://`/`https://`
//! - Mapping of `ureq::Error` onto the `AmError` transport-error contract:
//!   transient failures carry the retryable `(sidecar unreachable)` marker,
//!   terminal failures (`Tls`, `BadUri`, `Protocol`, and certificate-bearing
//!   `Io`) do not
//!
//! ## Does not own
//! - Retry classification (`embed::is_retryable` consumes this module's
//!   error-text substrings)
//! - Embed text assembly, redaction, or dimension gating (`embed`)
//! - Index storage or worker scheduling (`worker`)
//!
//! ## Boundary
//!
//! Changes to sidecar transport behavior should normally require changes here.
//! Changes to retry policy or index pipeline policy should not require changes here.

use clio_types::{AmError, ErrorCode, is_loopback_host};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Parsed `http://` or `https://host[:port][/path]` endpoint.
#[derive(Debug)]
pub struct HttpEndpoint {
    /// Host (no scheme, no port).
    pub host: String,
    /// TCP port (default 80 for `http://`, 443 for `https://`).
    pub port: u16,
    /// Request path starting with `/` (default `/`).
    pub path: String,
}

impl HttpEndpoint {
    /// Parse an `http://`/`https://` URL; fail closed on anything else.
    ///
    /// Bracketed IPv6 literals (`http://[::1]:8080`) are supported: the stored
    /// `host` keeps no brackets and the bracket content must parse as an IPv6
    /// address. Actual requests go through the original URL string, so the
    /// parsed fields are display/validation data only.
    ///
    /// # Errors
    /// Non-http/https scheme, malformed host/port.
    pub fn parse(url: &str) -> Result<Self, AmError> {
        let (scheme, rest) = if let Some(rest) = url.strip_prefix("http://") {
            ("http", rest)
        } else if let Some(rest) = url.strip_prefix("https://") {
            ("https", rest)
        } else {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("sidecar url must be http:// or https://, got `{url}`"),
            ));
        };
        let default_port = if scheme == "https" { 443 } else { 80 };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_owned()),
            None => (rest, "/".to_owned()),
        };
        let (host, port) = if let Some(after_open) = authority.strip_prefix('[') {
            // Bracketed IPv6 literal: "[host]" optionally followed by ":port".
            let Some(close) = after_open.find(']') else {
                return Err(AmError::new(
                    ErrorCode::ConfigCorrupt,
                    format!("sidecar url has malformed bracketed host `{authority}`"),
                ));
            };
            let host = &after_open[..close];
            if host.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(AmError::new(
                    ErrorCode::ConfigCorrupt,
                    format!("sidecar url has malformed bracketed host `{authority}`"),
                ));
            }
            let tail = &after_open[close + 1..];
            let port = match tail.strip_prefix(':') {
                Some(p) => p.parse::<u16>().map_err(|_| {
                    AmError::new(
                        ErrorCode::ConfigCorrupt,
                        format!("sidecar url has invalid port `{p}`"),
                    )
                })?,
                None if tail.is_empty() => default_port,
                None => {
                    return Err(AmError::new(
                        ErrorCode::ConfigCorrupt,
                        format!("sidecar url has malformed bracketed host `{authority}`"),
                    ));
                }
            };
            (host.to_owned(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (
                    h.to_owned(),
                    p.parse::<u16>().map_err(|_| {
                        AmError::new(
                            ErrorCode::ConfigCorrupt,
                            format!("sidecar url has invalid port `{p}`"),
                        )
                    })?,
                ),
                None => (authority.to_owned(), default_port),
            }
        };
        if host.is_empty() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "sidecar url has empty host",
            ));
        }
        Ok(Self { host, port, path })
    }
}

/// Refuse to send anything to a remote host over plain `http`.
///
/// A hosted embedding or reranking endpoint is sent the API key the operator
/// typed, as a request header, on every call, along with every piece of text
/// being embedded. Over plain `http` that crosses the network in the clear, so
/// a value that reached this point by a route the config rule did not cover — a
/// hand-edited file, a value stored before the rule existed — is stopped here
/// rather than obeyed. The config rule is the friendly half; this is the half
/// that makes it honest.
///
/// The check reads the host [`HttpEndpoint::parse`] already computed, not a
/// split of the string. `http://trusted@evil.example.com` parses to the host
/// `trusted@evil.example.com`, which is not loopback, so it is refused; a
/// hand-rolled split on `://` and `/` is exactly the sort of thing that lets
/// that through.
///
/// Loopback is exempt, so the local sidecars stay on `http`.
///
/// # Errors
/// The endpoint is plain `http` to a host that is not this machine.
fn require_encrypted_endpoint(url: &str, endpoint: &HttpEndpoint) -> Result<(), AmError> {
    // `parse` has already read the scheme, and it only accepts two. A `https`
    // endpoint is encrypted whatever the host is.
    if url.trim_start().starts_with("https://") {
        return Ok(());
    }
    if is_loopback_host(&endpoint.host) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "refusing to send to `{url}`: a remote endpoint must be https, because the API key \
             and every text sent to it would cross the network unencrypted. Change the scheme to \
             `https://{}`, or point it at a service on this machine with a loopback address such \
             as `http://127.0.0.1:<port>`",
            endpoint.host
        ),
    ))
}

/// Parse `url` and refuse a remote plain-`http` endpoint.
///
/// This is the one place both halves happen, so every send function below gets
/// the gate by calling it rather than by remembering to.
///
/// # Errors
/// The URL is not `http`/`https`, or it is plain `http` to a remote host.
pub(crate) fn gate(url: &str) -> Result<HttpEndpoint, AmError> {
    let endpoint = HttpEndpoint::parse(url)?;
    require_encrypted_endpoint(url, &endpoint)?;
    Ok(endpoint)
}

/// One-shot ureq agent: global deadline, statuses returned as responses (we map
/// them ourselves), redirects never followed. Configures custom root CA if
/// provided; otherwise defaults to WebPKI roots.
fn agent(timeout: Duration, ca_pem: Option<&[u8]>) -> Result<ureq::Agent, AmError> {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0);
    if let Some(pem) = ca_pem {
        let cert =
            ureq::tls::Certificate::from_pem(pem).map_err(|e| transport_error("tls config", e))?;
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::Specific(std::sync::Arc::new(vec![
                cert,
            ])))
            .build();
        builder = builder.tls_config(tls);
    }
    Ok(builder.build().into())
}

/// Map a `ureq::Error` onto the transport-error contract.
///
/// Non-2xx statuses become `HTTP <code>` text (retry classification in
/// `embed::is_retryable` matches on `HTTP 429`/`HTTP 5`). Transient transport
/// failures (Timeout, plain `Io`, ConnectionFailed, HostNotFound) always
/// contain `sidecar unreachable`, and connect-ish variants additionally say
/// `connect failed` (Timeout says `read timed out`) so they classify as
/// retryable. Terminal failures get neither the marker nor any retryable
/// substring, so `embed::is_retryable` returns false for them: client errors
/// (`Tls`, `BadUri`, `Protocol`) and certificate failures wrapped in `Io`
/// (ureq surfaces TLS verification errors as `Io` with `certificate` in the
/// message text).
fn transport_error(context: &str, err: ureq::Error) -> AmError {
    let (transient, reason) = match err {
        ureq::Error::StatusCode(code) => {
            return AmError::new(
                ErrorCode::Internal,
                format!("endpoint returned HTTP {code}"),
            );
        }
        ureq::Error::Timeout(_) => (true, "read timed out".to_owned()),
        ureq::Error::Io(e) => {
            let s = e.to_string();
            if s.contains("certificate") || s.contains("Certificate") {
                (false, format!("tls failure: {s}"))
            } else {
                (true, format!("connect failed: {s}"))
            }
        }
        ureq::Error::ConnectionFailed => (true, "connect failed".to_owned()),
        ureq::Error::HostNotFound => (true, "connect failed: host not found".to_owned()),
        ureq::Error::Tls(e) => (false, format!("tls failure: {e}")),
        ureq::Error::BadUri(u) => (false, format!("bad uri `{u}`")),
        ureq::Error::Protocol(e) => (false, format!("protocol error: {e}")),
        other => (true, format!("{other}")),
    };
    let marker = if transient {
        " (sidecar unreachable)"
    } else {
        ""
    };
    AmError::new(ErrorCode::Internal, format!("{context}{marker}: {reason}"))
}

/// Read the response body in bounded chunks, failing above the 64 MiB cap
/// (ureq does not cap response bodies itself).
fn read_capped(body: &mut ureq::Body) -> Result<Vec<u8>, AmError> {
    let mut out = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    let mut reader = body.as_reader();
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() > MAX_BODY_BYTES {
                    return Err(AmError::new(
                        ErrorCode::OutOfRange,
                        "response exceeds the 64 MiB safety cap",
                    ));
                }
            }
            Err(e) => {
                return Err(AmError::new(
                    ErrorCode::Internal,
                    format!("response read (sidecar unreachable): {e}"),
                ));
            }
        }
    }
}

/// POST a JSON body with an optional custom root CA certificate.
///
/// # Errors
/// DNS/connect/IO/TLS failures, non-2xx status, oversize bodies, non-JSON 2xx.
pub(crate) fn post_json_with_ca(
    url: &str,
    body: &Value,
    timeout: Duration,
    bearer: Option<&str>,
    ca_pem: Option<&[u8]>,
) -> Result<Value, AmError> {
    gate(url)?;
    let payload = serde_json::to_vec(body)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("body encode: {e}")))?;
    let agent = agent(timeout, ca_pem)?;
    let mut request = agent.post(url).header("content-type", "application/json");
    if let Some(key) = bearer {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let response = request
        .send(&payload)
        .map_err(|e| transport_error("request", e))?;
    let code = response.status().as_u16();
    let mut body = response.into_body();
    let bytes = read_capped(&mut body)?;
    parse_response_json(code, &bytes)
}

/// POST a JSON body and parse the JSON response.
///
/// # Errors
/// DNS/connect/IO/TLS failures, non-2xx status, oversize bodies, non-JSON 2xx.
pub fn post_json(
    url: &str,
    body: &Value,
    timeout: Duration,
    bearer: Option<&str>,
) -> Result<Value, AmError> {
    post_json_with_ca(url, body, timeout, bearer, None)
}

/// GET a URL and require a 2xx status (health probes).
///
/// # Errors
/// DNS/connect/IO/TLS failures, non-2xx status.
pub fn get_ok(url: &str, timeout: Duration, bearer: Option<&str>) -> Result<(), AmError> {
    gate(url)?;
    let agent = agent(timeout, None)?;
    let mut request = agent.get(url);
    if let Some(key) = bearer {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let response = request
        .call()
        .map_err(|e| transport_error("health probe", e))?;
    let code = response.status().as_u16();
    if (200..300).contains(&code) {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::Internal,
        format!("endpoint returned HTTP {code}"),
    ))
}

/// GET a URL and parse the JSON response.
///
/// # Errors
/// DNS/connect/IO/TLS failures, non-2xx status, oversize bodies, non-JSON 2xx.
pub fn get_json(url: &str, timeout: Duration, auth: Option<&str>) -> Result<Value, AmError> {
    gate(url)?;
    let agent = agent(timeout, None)?;
    let mut request = agent.get(url).header("accept", "application/json");
    if let Some(token) = auth {
        let val = if token.starts_with("Token ") || token.starts_with("Bearer ") {
            token.to_owned()
        } else {
            format!("Bearer {token}")
        };
        request = request.header("authorization", val);
    }
    let response = request.call().map_err(|e| transport_error("request", e))?;
    let code = response.status().as_u16();
    let mut body = response.into_body();
    let bytes = read_capped(&mut body)?;
    parse_response_json(code, &bytes)
}

/// Parse a TEI-style JSON response: non-2xx surfaces the status plus a short
/// body snippet, except 401/403 which omit the snippet (an external endpoint
/// can echo invalid credentials back, and those must never land in `AmError`
/// text); a 2xx non-JSON body fails closed.
fn parse_response_json(code: u16, body: &[u8]) -> Result<Value, AmError> {
    if !(200..300).contains(&code) {
        if matches!(code, 401 | 403) {
            return Err(AmError::new(
                ErrorCode::Internal,
                format!("endpoint returned HTTP {code}"),
            ));
        }
        let snippet = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(AmError::new(
            ErrorCode::Internal,
            format!("endpoint returned HTTP {code}: {}", snippet.trim()),
        ));
    }
    serde_json::from_slice(body).map_err(|e| {
        AmError::new(
            ErrorCode::Internal,
            format!("endpoint returned non-JSON body: {e}"),
        )
    })
}

#[cfg(test)]
mod http_unit_tests;

#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;

#[cfg(test)]
#[path = "http_tls_mock_tests.rs"]
mod http_tls_mock_tests;
