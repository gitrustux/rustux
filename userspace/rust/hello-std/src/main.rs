// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! A Rust program using std on Rustux (checklist step 10): printing,
//! arguments and environment, files and directories, threads with a
//! mutex and a channel, a child process with its output piped back, and
//! time. Prints "hello-std: N passed, M failed" at the end; the exit code
//! is the number of failures.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Checks {
    passed: u32,
    failed: u32,
}

impl Checks {
    fn check(&mut self, name: &str, ok: bool) {
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
            println!("  FAIL {name}");
        }
    }
}

fn main() {
    let mut c = Checks { passed: 0, failed: 0 };
    println!("Hello from Rust std on Rustux!");

    // Arguments and environment
    let args: Vec<String> = std::env::args().collect();
    println!("args: {args:?}");
    c.check("argv[0]", args.first().is_some_and(|a| a.ends_with("hello-std")));
    c.check("PATH is set", std::env::var("PATH").is_ok());
    std::env::set_var("RUSTUX_TEST", "1");
    c.check("set_var / var", std::env::var("RUSTUX_TEST").as_deref() == Ok("1"));
    c.check("current_dir", std::env::current_dir().is_ok_and(|d| d.is_absolute()));

    // Files and directories
    let dir = std::path::Path::new("/tmp/rust-test");
    let _ = std::fs::remove_dir_all(dir);
    c.check("create_dir_all", std::fs::create_dir_all(dir.join("a/b")).is_ok());
    c.check("write", std::fs::write(dir.join("a/f.txt"), b"written by Rust\n").is_ok());
    let s = std::fs::read_to_string(dir.join("a/f.txt")).unwrap_or_default();
    c.check("read back", s == "written by Rust\n");
    let mut f = std::fs::OpenOptions::new().append(true).open(dir.join("a/f.txt")).unwrap();
    c.check("append", f.write_all(b"more\n").is_ok());
    drop(f);
    let meta = std::fs::metadata(dir.join("a/f.txt"));
    c.check("metadata: size", meta.as_ref().is_ok_and(|m| m.len() == 21 && m.is_file()));
    c.check("metadata: modified", meta.is_ok_and(|m| m.modified().is_ok()));
    let names: Vec<String> = std::fs::read_dir(dir.join("a"))
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    c.check("read_dir", names.len() == 2 && names.contains(&"b".into()) && names.contains(&"f.txt".into()));
    c.check("rename", std::fs::rename(dir.join("a/f.txt"), dir.join("g.txt")).is_ok());
    c.check("missing file -> NotFound",
            std::fs::read(dir.join("a/f.txt")).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound));
    c.check("canonicalize", std::fs::canonicalize("/tmp/rust-test/a/../g.txt").is_ok_and(|p| p.to_str() == Some("/tmp/rust-test/g.txt")));
    c.check("/dev/null", std::fs::write("/dev/null", b"gone").is_ok() && std::fs::read("/dev/null").is_ok_and(|v| v.is_empty()));
    c.check("remove_dir_all", std::fs::remove_dir_all(dir).is_ok() && !dir.exists());

    // Threads
    let squares: Vec<u64> = (0..4u64).map(|i| std::thread::spawn(move || i * i)).collect::<Vec<_>>()
        .into_iter().map(|t| t.join().unwrap()).collect();
    println!("threads: {squares:?}");
    c.check("spawn and join", squares == [0, 1, 4, 9]);
    let total = Arc::new(Mutex::new(0u64));
    let workers: Vec<_> = (0..8).map(|_| {
        let total = total.clone();
        std::thread::spawn(move || {
            for _ in 0..1000 {
                *total.lock().unwrap() += 1;
            }
        })
    }).collect();
    for w in workers {
        w.join().unwrap();
    }
    c.check("mutex across 8 threads", *total.lock().unwrap() == 8000);
    let (tx, rx) = mpsc::channel();
    for i in 0..4 {
        let tx = tx.clone();
        std::thread::Builder::new().name(format!("sender{i}")).spawn(move || {
            tx.send(i).unwrap();
        }).unwrap();
    }
    drop(tx);
    let mut got: Vec<i32> = rx.iter().collect();
    got.sort();
    c.check("channel", got == [0, 1, 2, 3]);
    thread_local! { static COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    COUNT.with(|n| n.set(5));
    let other = std::thread::spawn(|| COUNT.with(|n| n.get())).join().unwrap();
    c.check("thread-local storage", COUNT.with(|n| n.get()) == 5 && other == 0);

    // Time
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    let slept = t0.elapsed();
    c.check("sleep 50 ms", slept >= Duration::from_millis(50) && slept < Duration::from_millis(500));
    c.check("system time after 2024",
            SystemTime::now().duration_since(UNIX_EPOCH).is_ok_and(|d| d.as_secs() > 1_704_067_200));

    // Child processes
    match Command::new("/bin/echo").arg("from a child").output() {
        Ok(out) => {
            print!("child said: {}", String::from_utf8_lossy(&out.stdout));
            c.check("Command::output", out.status.success() && out.stdout == b"from a child\n");
        }
        Err(e) => {
            println!("  Command::output: {e}");
            c.check("Command::output", false);
        }
    }
    let status = Command::new("false").status();
    c.check("exit status of false (found on PATH)", status.is_ok_and(|s| s.code() == Some(1)));
    let child = Command::new("/bin/upper").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn();
    let ok = child.is_ok_and(|mut ch| {
        ch.stdin.take().unwrap().write_all(b"piped through\n").unwrap();
        let mut s = String::new();
        ch.stdout.take().unwrap().read_to_string(&mut s).unwrap();
        ch.wait().is_ok_and(|st| st.success()) && s == "PIPED THROUGH\n"
    });
    c.check("stdin and stdout pipes", ok);
    let pwd = Command::new("/bin/env").current_dir("/tmp").env_clear().output();
    c.check("current_dir and env_clear for a child",
            pwd.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "PWD=/tmp"));
    c.check("missing program -> error", Command::new("/bin/no-such-program").status().is_err());

    // Collections (hashing needs random numbers)
    let mut m = HashMap::new();
    m.insert("rustux", 10);
    c.check("HashMap", m.get("rustux") == Some(&10));

    println!("hello-std: {} passed, {} failed", c.passed, c.failed);
    std::process::exit(c.failed as i32);
}
