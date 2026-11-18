// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Downloads: https:// and http:// (ureq on rustls with Mozilla's root
//! certificates, as curl), and file:// (a repository on a disk, the
//! live medium's from step E3). Certificates for a private repository
//! can be added in /etc/rpg/ca-certificates.pem (PEM, one or more).
//! HTTPS keeps the download private; what makes it trustworthy is the
//! signature of the repository's Release file (repo.rs).

use std::sync::Arc;
use std::time::Duration;

pub enum Error {
    /// The server has no such file (HTTP 404, or no file there)
    NotFound,
    Other(String),
}

impl Error {
    pub fn text(&self) -> String {
        match self {
            Error::NotFound => "not there (404)".into(),
            Error::Other(e) => e.clone(),
        }
    }
}

pub struct Fetcher {
    agent: Option<ureq::Agent>,
    extra_ca: std::path::PathBuf,
    setup_error: Option<String>,
}

impl Fetcher {
    /// `extra_ca`: the system's /etc/rpg/ca-certificates.pem (under --root)
    pub fn new(extra_ca: std::path::PathBuf) -> Fetcher {
        Fetcher { agent: None, extra_ca, setup_error: None }
    }

    fn agent(&mut self) -> Result<&ureq::Agent, Error> {
        if let Some(e) = &self.setup_error {
            return Err(Error::Other(e.clone()));
        }
        if self.agent.is_none() {
            match tls(&self.extra_ca) {
                Ok(cfg) => {
                    self.agent = Some(
                        ureq::AgentBuilder::new()
                            .tls_config(cfg)
                            .user_agent(concat!("rpg/", env!("CARGO_PKG_VERSION")))
                            .timeout_connect(Duration::from_secs(20))
                            .timeout_read(Duration::from_secs(60))
                            .build(),
                    );
                }
                Err(e) => {
                    self.setup_error = Some(e.clone());
                    return Err(Error::Other(e));
                }
            }
        }
        Ok(self.agent.as_ref().unwrap())
    }

    /// The whole file at `url`
    pub fn get(&mut self, url: &str) -> Result<Vec<u8>, Error> {
        if let Some(path) = url.strip_prefix("file://") {
            return match std::fs::read(path) {
                Ok(d) => Ok(d),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound),
                Err(e) => Err(Error::Other(format!("{path}: {e}"))),
            };
        }
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(Error::Other(format!("{url}: rpg fetches https://, http:// and file:// addresses")));
        }
        let agent = self.agent()?.clone();
        match agent.get(url).call() {
            Ok(r) => {
                let mut v = Vec::new();
                use std::io::Read;
                r.into_reader().take(1 << 31).read_to_end(&mut v).map_err(|e| Error::Other(format!("reading: {e}")))?;
                Ok(v)
            }
            Err(ureq::Error::Status(404, _)) | Err(ureq::Error::Status(410, _)) => Err(Error::NotFound),
            Err(ureq::Error::Status(code, r)) => Err(Error::Other(format!("HTTP {code} {}", r.status_text()))),
            Err(ureq::Error::Transport(t)) => Err(Error::Other(transport(&t))),
        }
    }
}

fn transport(t: &ureq::Transport) -> String {
    use ureq::ErrorKind::*;
    let what = match t.kind() {
        Dns => "the name could not be looked up",
        ConnectionFailed => "could not connect",
        Io => "the connection failed",
        InvalidUrl | UnknownScheme => "not an address rpg understands",
        _ => "",
    };
    let detail = t.to_string();
    if detail.contains("certificate") || detail.contains("UnknownIssuer") {
        return format!("the server's certificate is not trusted ({detail}); a private repository's CA goes in /etc/rpg/ca-certificates.pem");
    }
    if what.is_empty() {
        detail
    } else {
        format!("{what} ({detail})")
    }
}

fn tls(extra: &std::path::Path) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(|e| e.to_string())?;
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    if let Ok(pem) = std::fs::read(extra) {
        for c in rustls_pemfile::certs(&mut &pem[..]) {
            let c = c.map_err(|e| format!("{}: {e}", extra.display()))?;
            roots.add(c).map_err(|e| format!("{}: {e}", extra.display()))?;
        }
    }
    Ok(Arc::new(builder.with_root_certificates(roots).with_no_client_auth()))
}
