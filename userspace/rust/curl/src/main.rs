// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! curl: HTTP and HTTPS from the command line (network step D3,
//! docs/NETWORK.md, man curl), the familiar options of curl's. On ureq
//! and rustls (ring), with Mozilla's root certificates built in
//! (webpki-roots); names through the C library's resolver (DNS).

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

const VERSION: &str = "curl 8.0.0-rustica (x86_64-rustica) ureq/2 rustls/0.23";

struct Opts {
    urls: Vec<String>,
    output: Option<String>,
    remote_name: bool,
    silent: bool,
    show_error: bool,
    follow: bool,
    head: bool,
    include: bool,
    fail: bool,
    verbose: bool,
    insecure: bool,
    headers: Vec<(String, String)>,
    data: Option<Vec<u8>>,
    method: Option<String>,
    max_time: Option<f64>,
    connect_timeout: Option<f64>,
    agent: String,
    cacert: Option<String>,
    write_out: Option<String>,
    user: Option<String>,
    max_redirs: u32,
}

fn usage() -> ! {
    eprintln!(
        "usage: curl [options] URL...\n\
         \x20 -o, --output FILE      write to FILE        -O, --remote-name   the URL's file name\n\
         \x20 -L, --location         follow redirects     -I, --head          headers only\n\
         \x20 -i, --include          show the headers     -f, --fail          fail on HTTP errors\n\
         \x20 -s, --silent           no messages          -S, --show-error    errors even with -s\n\
         \x20 -v, --verbose          what happens         -k, --insecure      do not check certificates\n\
         \x20 -H, --header 'N: V'    a request header     -d, --data DATA     POST DATA (@FILE)\n\
         \x20 -X, --request METHOD   -A, --user-agent UA  -u, --user USER:PASSWORD\n\
         \x20 -m, --max-time SECONDS --connect-timeout SECONDS  --cacert FILE (PEM)\n\
         \x20 -w, --write-out FORMAT (%{{http_code}}, %{{size_download}}, %{{url_effective}}, %{{content_type}}, %{{time_total}})\n\
         \x20 --max-redirs N  -V, --version"
    );
    std::process::exit(2)
}

fn parse() -> Opts {
    let mut o = Opts {
        urls: vec![],
        output: None,
        remote_name: false,
        silent: false,
        show_error: false,
        follow: false,
        head: false,
        include: false,
        fail: false,
        verbose: false,
        insecure: false,
        headers: vec![],
        data: None,
        method: None,
        max_time: None,
        connect_timeout: None,
        agent: "curl/8.0.0-rustica".into(),
        cacert: None,
        write_out: None,
        user: None,
        max_redirs: 50,
    };
    let mut args = std::env::args().skip(1).peekable();
    let val = |name: &str, args: &mut std::iter::Peekable<std::iter::Skip<std::env::Args>>| -> String {
        args.next().unwrap_or_else(|| {
            eprintln!("curl: option {name}: requires parameter");
            std::process::exit(2)
        })
    };
    while let Some(a) = args.next() {
        // Short options may be grouped: -sSL
        if a.starts_with('-') && !a.starts_with("--") && a.len() > 2 {
            let flags: Vec<char> = a[1..].chars().collect();
            if flags.iter().all(|c| "sSLIifvkOV".contains(*c)) {
                for c in flags {
                    short_flag(&mut o, c);
                }
                continue;
            }
        }
        match a.as_str() {
            "-o" | "--output" => o.output = Some(val(&a, &mut args)),
            "-H" | "--header" => {
                let h = val(&a, &mut args);
                if let Some((n, v)) = h.split_once(':') {
                    o.headers.push((n.trim().to_string(), v.trim().to_string()));
                }
            }
            "-d" | "--data" | "--data-binary" | "--data-raw" => {
                let d = val(&a, &mut args);
                let bytes = if let Some(f) = d.strip_prefix('@').filter(|_| a != "--data-raw") {
                    std::fs::read(f).unwrap_or_else(|e| {
                        eprintln!("curl: {f}: {e}");
                        std::process::exit(26)
                    })
                } else {
                    d.into_bytes()
                };
                let mut all = o.data.take().unwrap_or_default();
                if !all.is_empty() {
                    all.push(b'&');
                }
                all.extend(bytes);
                o.data = Some(all);
            }
            "-X" | "--request" => o.method = Some(val(&a, &mut args)),
            "-m" | "--max-time" => o.max_time = val(&a, &mut args).parse().ok(),
            "--connect-timeout" => o.connect_timeout = val(&a, &mut args).parse().ok(),
            "-A" | "--user-agent" => o.agent = val(&a, &mut args),
            "--cacert" => o.cacert = Some(val(&a, &mut args)),
            "-w" | "--write-out" => o.write_out = Some(val(&a, &mut args)),
            "-u" | "--user" => o.user = Some(val(&a, &mut args)),
            "--max-redirs" => o.max_redirs = val(&a, &mut args).parse().unwrap_or(50),
            "--remote-name" => o.remote_name = true,
            "--silent" => o.silent = true,
            "--show-error" => o.show_error = true,
            "--location" => o.follow = true,
            "--head" => o.head = true,
            "--include" => o.include = true,
            "--fail" => o.fail = true,
            "--verbose" => o.verbose = true,
            "--insecure" => o.insecure = true,
            "--compressed" => {}
            "--version" => short_flag(&mut o, 'V'),
            "-h" | "--help" => usage(),
            s if s.starts_with('-') && s.len() == 2 => short_flag(&mut o, s.chars().nth(1).unwrap()),
            s if s.starts_with('-') => {
                eprintln!("curl: option {s}: is unknown");
                usage()
            }
            _ => o.urls.push(a),
        }
    }
    if o.urls.is_empty() {
        eprintln!("curl: no URL specified");
        usage();
    }
    o
}

fn short_flag(o: &mut Opts, c: char) {
    match c {
        's' => o.silent = true,
        'S' => o.show_error = true,
        'L' => o.follow = true,
        'I' => o.head = true,
        'i' => o.include = true,
        'f' => o.fail = true,
        'v' => o.verbose = true,
        'k' => o.insecure = true,
        'O' => o.remote_name = true,
        'V' => {
            println!("{VERSION}\nProtocols: http https\nFeatures: gzip SSL");
            std::process::exit(0)
        }
        _ => {
            eprintln!("curl: option -{c}: is unknown");
            usage()
        }
    }
}

// -k: any certificate will do
#[derive(Debug)]
struct NoCheck(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCheck {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls(o: &Opts) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?;
    let cfg = if o.insecure {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(NoCheck(provider))).with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        if let Some(f) = &o.cacert {
            let pem = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
            let mut n = 0;
            for c in rustls_pemfile::certs(&mut &pem[..]) {
                let c = c.map_err(|e| format!("{f}: {e}"))?;
                roots.add(c).map_err(|e| format!("{f}: {e}"))?;
                n += 1;
            }
            if n == 0 {
                return Err(format!("{f}: no certificates in it"));
            }
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

fn base64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

struct Fail {
    code: i32,
    msg: String,
}

fn err(code: i32, msg: impl Into<String>) -> Fail {
    Fail { code, msg: msg.into() }
}

// curl's exit codes for ureq's errors
fn transport(t: &ureq::Transport, url: &str) -> Fail {
    use ureq::ErrorKind::*;
    // The whole chain: rustls's reason is deep in it
    let mut msg = t.to_string();
    let mut src = std::error::Error::source(t);
    while let Some(e) = src {
        msg.push_str(": ");
        msg.push_str(&e.to_string());
        src = e.source();
    }
    let lower = msg.to_lowercase();
    if lower.contains("invalid peer certificate") || lower.contains("unknownissuer") || lower.contains("certificate") && lower.contains("not valid") {
        let why = msg.rsplit(": ").next().unwrap_or(&msg).to_string();
        return err(60, format!("SSL certificate problem: {why}"));
    }
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', ':']).next().unwrap_or("");
    match t.kind() {
        Dns => err(6, format!("Could not resolve host: {host}")),
        ConnectionFailed => {
            if lower.contains("timed out") {
                err(28, format!("Failed to connect to {host}: Connection timed out"))
            } else {
                err(7, format!("Failed to connect to {host}: {}", t.message().unwrap_or("Connection refused")))
            }
        }
        InvalidUrl | UnknownScheme => err(3, format!("URL rejected: {msg}")),
        TooManyRedirects => err(47, "Maximum redirects followed"),
        Io if lower.contains("timed out") || lower.contains("timeout") => err(28, "Operation timed out"),
        _ if lower.contains("certificate") || lower.contains("invalidcertificate") => err(60, format!("SSL certificate problem: {msg}")),
        _ if lower.contains("tls") || lower.contains("handshake") || lower.contains("alert") => err(35, format!("TLS connect error: {msg}")),
        _ => err(56, format!("Failure when receiving data from the peer: {msg}")),
    }
}

fn remote_file(url: &str) -> String {
    let path = url.split("://").nth(1).unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    if path.contains('/') && !last.is_empty() {
        last.to_string()
    } else {
        "index.html".into()
    }
}

fn fetch(o: &Opts, url: &str, agent: &ureq::Agent) -> Result<(), Fail> {
    let url = if url.contains("://") { url.to_string() } else { format!("http://{url}") };
    let method = o.method.clone().unwrap_or_else(|| if o.head { "HEAD".into() } else if o.data.is_some() { "POST".into() } else { "GET".into() });
    let mut req = agent.request(&method, &url);
    let mut has_ct = false;
    for (n, v) in &o.headers {
        has_ct |= n.eq_ignore_ascii_case("content-type");
        req = req.set(n, v);
    }
    if let Some(u) = &o.user {
        let up = if u.contains(':') { u.clone() } else { format!("{u}:") };
        req = req.set("Authorization", &format!("Basic {}", base64(up.as_bytes())));
    }
    if o.verbose {
        eprintln!("> {method} {url}");
        eprintln!("> User-Agent: {}", o.agent);
        for (n, v) in &o.headers {
            eprintln!("> {n}: {v}");
        }
    }
    let started = Instant::now();
    let res = match &o.data {
        Some(d) => {
            if !has_ct {
                req = req.set("Content-Type", "application/x-www-form-urlencoded");
            }
            req.send_bytes(d)
        }
        None => req.call(),
    };
    let resp = match res {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(transport(&t, &url)),
    };
    let status = resp.status();
    if o.verbose {
        eprintln!("< {} {} {}", resp.http_version(), status, resp.status_text());
        for h in resp.headers_names() {
            if let Some(v) = resp.header(&h) {
                eprintln!("< {h}: {v}");
            }
        }
    }
    if o.fail && status >= 400 {
        return Err(err(22, format!("The requested URL returned error: {status}")));
    }
    // Where the body goes
    let mut out: Box<dyn Write> = match (&o.output, o.remote_name) {
        (Some(f), _) if f != "-" => Box::new(std::fs::File::create(f).map_err(|e| err(23, format!("{f}: {e}")))?),
        (None, true) => {
            let f = remote_file(&url);
            Box::new(std::fs::File::create(&f).map_err(|e| err(23, format!("{f}: {e}")))?)
        }
        _ => Box::new(std::io::stdout().lock()),
    };
    if o.include || o.head {
        let mut h = format!("{} {} {}\r\n", resp.http_version(), status, resp.status_text());
        for n in resp.headers_names() {
            if let Some(v) = resp.header(&n) {
                h.push_str(&format!("{n}: {v}\r\n"));
            }
        }
        h.push_str("\r\n");
        out.write_all(h.as_bytes()).map_err(|e| err(23, e.to_string()))?;
    }
    let content_type = resp.content_type().to_string();
    let effective = resp.get_url().to_string();
    let mut size = 0u64;
    if method != "HEAD" {
        let mut r = resp.into_reader();
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(err(28, "Operation timed out"))
                }
                Err(e) => return Err(err(56, format!("Failure when receiving data from the peer: {e}"))),
            };
            out.write_all(&buf[..n]).map_err(|e| err(23, e.to_string()))?;
            size += n as u64;
        }
    }
    out.flush().map_err(|e| err(23, e.to_string()))?;
    drop(out);
    if let Some(w) = &o.write_out {
        let s = w
            .replace("%{http_code}", &format!("{status:03}"))
            .replace("%{response_code}", &format!("{status:03}"))
            .replace("%{size_download}", &size.to_string())
            .replace("%{url_effective}", &effective)
            .replace("%{content_type}", &content_type)
            .replace("%{time_total}", &format!("{:.6}", started.elapsed().as_secs_f64()))
            .replace("\\n", "\n")
            .replace("\\t", "\t");
        print!("{s}");
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

fn main() {
    let o = parse();
    let cfg = match tls(&o) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("curl: (77) error setting certificate file: {e}");
            std::process::exit(77)
        }
    };
    let mut b = ureq::AgentBuilder::new().tls_config(cfg).user_agent(&o.agent).redirects(if o.follow { o.max_redirs } else { 0 });
    if let Some(t) = o.max_time {
        b = b.timeout(Duration::from_secs_f64(t.max(0.001)));
    }
    b = b.timeout_connect(Duration::from_secs_f64(o.connect_timeout.unwrap_or(30.0).max(0.001)));
    let agent = b.build();
    let mut code = 0;
    for u in &o.urls {
        if let Err(f) = fetch(&o, u, &agent) {
            if !o.silent || o.show_error {
                eprintln!("curl: ({}) {}", f.code, f.msg);
            }
            code = f.code;
        }
    }
    std::process::exit(code)
}
