// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! tar and gzip/gunzip/zcat, on the `tar` and `flate2` crates

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

fn err(e: &io::Error) -> String {
    let s = e.to_string();
    s.split(" (os error").next().unwrap_or(&s).to_string()
}

const TAR_HELP: &str = "Usage: tar -c|-x|-t [OPTION]... [FILE]...
Create, extract or list a tar archive.

  -c, --create          create an archive from FILEs (directories with what is in them)
  -x, --extract         extract FILEs (everything without FILE)
  -t, --list            list what is in the archive
  -f, --file=ARCHIVE    the archive (- or none: standard input/output)
  -z, --gzip            compressed with gzip (.tar.gz, .tgz)
  -C, --directory=DIR   change to DIR first
  -v, --verbose         list the files as they are processed
  -O, --to-stdout       extract to standard output
      --exclude=GLOB    leave out files whose names match GLOB
      --json            with -t: one JSON object per entry

The options may be bundled without a dash: tar czf a.tar.gz dir";

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    None,
    Create,
    Extract,
    List,
}

pub fn tar_main(args: Vec<OsString>) -> i32 {
    let mut args: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    // "tar czf x" is "tar -czf x"
    if let Some(first) = args.first() {
        if !first.starts_with('-') && !first.is_empty() {
            args[0] = format!("-{first}");
        }
    }
    let mut mode = Mode::None;
    let mut archive: Option<String> = None;
    let mut gz = false;
    let mut dir: Option<String> = None;
    let mut verbose = false;
    let mut to_stdout = false;
    let mut json = false;
    let mut exclude: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if let Some(long) = a.strip_prefix("--") {
            let (k, v) = match long.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (long, None),
            };
            let mut val = || -> Option<String> {
                v.clone().or_else(|| {
                    i += 1;
                    args.get(i).cloned()
                })
            };
            match k {
                "help" => {
                    println!("{TAR_HELP}");
                    return 0;
                }
                "create" => mode = Mode::Create,
                "extract" | "get" => mode = Mode::Extract,
                "list" => mode = Mode::List,
                "file" => archive = val(),
                "gzip" | "gunzip" => gz = true,
                "directory" => dir = val(),
                "verbose" => verbose = true,
                "to-stdout" => to_stdout = true,
                "json" => json = true,
                "exclude" => exclude.extend(val()),
                _ => {
                    eprintln!("tar: unknown option --{k} (see tar --help)");
                    return 2;
                }
            }
        } else if a.starts_with('-') && a.len() > 1 {
            let chars: Vec<char> = a[1..].chars().collect();
            for (k, c) in chars.iter().enumerate() {
                match c {
                    'c' => mode = Mode::Create,
                    'x' => mode = Mode::Extract,
                    't' => mode = Mode::List,
                    'z' => gz = true,
                    'v' => verbose = true,
                    'O' => to_stdout = true,
                    'p' | 'o' => {}
                    'f' | 'C' => {
                        let rest: String = chars[k + 1..].iter().collect();
                        let v = if !rest.is_empty() {
                            Some(rest)
                        } else {
                            i += 1;
                            args.get(i).cloned()
                        };
                        if *c == 'f' { archive = v } else { dir = v }
                        break;
                    }
                    _ => {
                        eprintln!("tar: unknown option -{c} (see tar --help)");
                        return 2;
                    }
                }
            }
        } else {
            files.push(a);
        }
        i += 1;
    }
    let archive = archive.filter(|a| a != "-");
    if !gz {
        gz = archive.as_deref().is_some_and(|a| a.ends_with(".gz") || a.ends_with(".tgz")) && mode != Mode::Create
            || archive.as_deref().is_some_and(|a| (a.ends_with(".tar.gz") || a.ends_with(".tgz")) && mode == Mode::Create);
    }
    let excluded = |p: &Path| {
        let s = p.to_string_lossy();
        exclude.iter().any(|g| crate::util::glob_match(g, &s) || crate::util::glob_match(g, crate::util::base_name(&s)))
    };
    let result = match mode {
        Mode::None => {
            eprintln!("tar: say -c, -x or -t (see tar --help)");
            return 2;
        }
        Mode::Create => {
            if files.is_empty() {
                eprintln!("tar: nothing to put in the archive");
                return 2;
            }
            // The archive is created before changing directory
            let out: Box<dyn Write> = match &archive {
                Some(a) => match File::create(a) {
                    Ok(f) => Box::new(BufWriter::new(f)),
                    Err(e) => {
                        eprintln!("tar: {a}: {}", err(&e));
                        return 2;
                    }
                },
                None => Box::new(io::stdout()),
            };
            if let Some(d) = &dir {
                if let Err(e) = std::env::set_current_dir(d) {
                    eprintln!("tar: {d}: {}", err(&e));
                    return 2;
                }
            }
            let out: Box<dyn Write> = if gz { Box::new(flate2::write::GzEncoder::new(out, flate2::Compression::default())) } else { out };
            create(out, &files, verbose, &excluded)
        }
        Mode::Extract | Mode::List => {
            let input: Box<dyn Read> = match &archive {
                Some(a) => match File::open(a) {
                    Ok(f) => Box::new(BufReader::new(f)),
                    Err(e) => {
                        eprintln!("tar: {a}: {}", err(&e));
                        return 2;
                    }
                },
                None => Box::new(io::stdin()),
            };
            let input: Box<dyn Read> = if gz { Box::new(flate2::read::GzDecoder::new(input)) } else { input };
            if let Some(d) = &dir {
                if let Err(e) = std::env::set_current_dir(d) {
                    eprintln!("tar: {d}: {}", err(&e));
                    return 2;
                }
            }
            read_archive(input, mode == Mode::List, &files, verbose, to_stdout, json, &excluded)
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tar: {e}");
            2
        }
    }
}

fn create(out: Box<dyn Write>, files: &[String], verbose: bool, excluded: &dyn Fn(&Path) -> bool) -> Result<(), String> {
    let mut b = tar::Builder::new(out);
    b.follow_symlinks(false);
    fn add(b: &mut tar::Builder<Box<dyn Write>>, path: &Path, verbose: bool, excluded: &dyn Fn(&Path) -> bool) -> Result<(), String> {
        if excluded(path) {
            return Ok(());
        }
        let meta = std::fs::metadata(path).map_err(|e| format!("{}: {}", path.display(), err(&e)))?;
        // Stored without a leading /
        let name: PathBuf = path.components().filter(|c| !matches!(c, std::path::Component::RootDir)).collect();
        if verbose {
            eprintln!("{}{}", name.display(), if meta.is_dir() { "/" } else { "" });
        }
        if meta.is_dir() {
            if !name.as_os_str().is_empty() && name != Path::new(".") {
                b.append_dir(&name, path).map_err(|e| format!("{}: {}", path.display(), err(&e)))?;
            }
            let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
                .map_err(|e| format!("{}: {}", path.display(), err(&e)))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            entries.sort();
            for e in entries {
                add(b, &e, verbose, excluded)?;
            }
        } else {
            let mut f = File::open(path).map_err(|e| format!("{}: {}", path.display(), err(&e)))?;
            b.append_file(&name, &mut f).map_err(|e| format!("{}: {}", path.display(), err(&e)))?;
        }
        Ok(())
    }
    for f in files {
        add(&mut b, Path::new(f), verbose, excluded)?;
    }
    let out = b.into_inner().map_err(|e| err(&e))?;
    finish(out)
}

// A gzip stream must write its trailer: dropping the encoder does that, but
// errors would be lost, so flush explicitly first
fn finish(mut out: Box<dyn Write>) -> Result<(), String> {
    out.flush().map_err(|e| err(&e))
}

fn read_archive(
    input: Box<dyn Read>,
    list: bool,
    only: &[String],
    verbose: bool,
    to_stdout: bool,
    json: bool,
    excluded: &dyn Fn(&Path) -> bool,
) -> Result<(), String> {
    let mut a = tar::Archive::new(input);
    a.set_preserve_permissions(true);
    let mut seen = vec![false; only.len()];
    for entry in a.entries().map_err(|e| err(&e))? {
        let mut entry = entry.map_err(|e| err(&e))?;
        let path = entry.path().map_err(|e| err(&e))?.into_owned();
        let p = path.to_string_lossy().trim_end_matches('/').to_string();
        if !only.is_empty() {
            let hit = only.iter().position(|o| {
                let o = o.trim_end_matches('/');
                p == o || p.starts_with(&format!("{o}/"))
            });
            match hit {
                Some(k) => seen[k] = true,
                None => continue,
            }
        }
        if excluded(&path) {
            continue;
        }
        let h = entry.header();
        let size = h.size().unwrap_or(0);
        let is_dir = h.entry_type().is_dir();
        if list {
            if json {
                println!(
                    "{{\"path\":{},\"type\":\"{}\",\"size\":{},\"mode\":\"{:o}\",\"mtime\":{}}}",
                    crate::util::json_str(&p),
                    if is_dir { "directory" } else { "file" },
                    size,
                    h.mode().unwrap_or(0) & 0o7777,
                    h.mtime().unwrap_or(0)
                );
            } else if verbose {
                println!(
                    "{}{:o} {:>10} {}{}",
                    if is_dir { 'd' } else { '-' },
                    h.mode().unwrap_or(0) & 0o7777,
                    size,
                    p,
                    if is_dir { "/" } else { "" }
                );
            } else {
                println!("{p}{}", if is_dir { "/" } else { "" });
            }
            continue;
        }
        if verbose {
            eprintln!("{p}");
        }
        if to_stdout {
            if !is_dir {
                io::copy(&mut entry, &mut io::stdout()).map_err(|e| err(&e))?;
            }
            continue;
        }
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            eprintln!("tar: {p}: skipped (outside the current directory)");
            continue;
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), err(&e)))?;
            }
        }
        entry.unpack(&path).map_err(|e| format!("{p}: {}", err(&e)))?;
    }
    for (k, s) in seen.iter().enumerate() {
        if !s {
            return Err(format!("{}: not in the archive", only[k]));
        }
    }
    Ok(())
}

const GZIP_HELP: &str = "Usage: gzip [OPTION]... [FILE]...
Compress FILEs (each FILE becomes FILE.gz), or standard input to standard output.

  -d, --decompress   decompress (as gunzip)
  -c, --stdout       write to standard output, keep the files
  -k, --keep         keep the input files
  -f, --force        overwrite existing output files
  -l, --list         list compressed and uncompressed sizes
  -t, --test         check compressed files
  -v, --verbose      say how much each file shrank
  -1 ... -9          faster ... smaller (default 6)

gunzip is gzip -d, zcat is gzip -dc.";

pub fn gzip_main(name: &str, args: Vec<OsString>) -> i32 {
    let args: Vec<String> = args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let mut decompress = name != "gzip";
    let mut to_stdout = name == "zcat";
    let mut keep = false;
    let mut force = false;
    let mut list = false;
    let mut test = false;
    let mut verbose = false;
    let mut level = 6u32;
    let mut files = Vec::new();
    for a in &args {
        match a.as_str() {
            "--help" => {
                println!("{GZIP_HELP}");
                return 0;
            }
            "--decompress" | "--uncompress" => decompress = true,
            "--stdout" | "--to-stdout" => to_stdout = true,
            "--keep" => keep = true,
            "--force" => force = true,
            "--list" => list = true,
            "--test" => test = true,
            "--verbose" => verbose = true,
            "--fast" => level = 1,
            "--best" => level = 9,
            "-" => files.push(a.clone()),
            s if s.starts_with('-') => {
                for c in s[1..].chars() {
                    match c {
                        'd' => decompress = true,
                        'c' => to_stdout = true,
                        'k' => keep = true,
                        'f' => force = true,
                        'l' => list = true,
                        't' => test = true,
                        'v' => verbose = true,
                        'n' | 'N' | 'q' => {}
                        d if d.is_ascii_digit() => level = d.to_digit(10).unwrap().clamp(1, 9),
                        _ => {
                            eprintln!("{name}: unknown option -{c} (see {name} --help)");
                            return 2;
                        }
                    }
                }
            }
            _ => files.push(a.clone()),
        }
    }
    if files.is_empty() {
        files.push("-".into());
    }
    let mut status = 0;
    for f in &files {
        if let Err(e) = gzip_one(name, f, decompress || list || test, to_stdout || f == "-", keep, force, list, test, verbose, level) {
            eprintln!("{name}: {e}");
            status = 1;
        }
    }
    status
}

#[allow(clippy::too_many_arguments)]
fn gzip_one(
    name: &str,
    f: &str,
    decompress: bool,
    to_stdout: bool,
    keep: bool,
    force: bool,
    list: bool,
    test: bool,
    verbose: bool,
    level: u32,
) -> Result<(), String> {
    let input: Box<dyn Read> = if f == "-" {
        Box::new(io::stdin())
    } else {
        Box::new(BufReader::new(File::open(f).map_err(|e| format!("{f}: {}", err(&e)))?))
    };
    if !to_stdout && !list && !test && f != "-" {
        if decompress && !(f.ends_with(".gz") || f.ends_with(".tgz")) {
            return Err(format!("{f}: unknown suffix -- ignored"));
        }
        if !decompress && f.ends_with(".gz") {
            return Err(format!("{f} already has .gz suffix -- unchanged"));
        }
    }
    let out_name = if f.ends_with(".tgz") {
        format!("{}.tar", &f[..f.len() - 4])
    } else if decompress {
        f.trim_end_matches(".gz").to_string()
    } else {
        format!("{f}.gz")
    };
    let in_size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
    if list || test {
        let mut d = flate2::read::GzDecoder::new(input);
        let n = io::copy(&mut d, &mut io::sink()).map_err(|e| format!("{f}: {}", err(&e)))?;
        if list {
            println!("{:>12} {:>12} {:>6.1}% {}", in_size, n, if n > 0 { 100.0 - in_size as f64 * 100.0 / n as f64 } else { 0.0 }, out_name);
        } else if verbose {
            println!("{f}: OK");
        }
        return Ok(());
    }
    let out: Box<dyn Write> = if to_stdout {
        Box::new(io::stdout())
    } else {
        if !force && Path::new(&out_name).exists() {
            return Err(format!("{out_name} already exists (use -f to overwrite)"));
        }
        Box::new(BufWriter::new(File::create(&out_name).map_err(|e| format!("{out_name}: {}", err(&e)))?))
    };
    let mut written = 0u64;
    let r: io::Result<u64> = if decompress {
        let mut d = flate2::read::MultiGzDecoder::new(input);
        let mut out = out;
        let n = io::copy(&mut d, &mut out);
        n.and_then(|n| out.flush().map(|_| n))
    } else {
        let mut e = flate2::write::GzEncoder::new(out, flate2::Compression::new(level));
        let mut input = input;
        io::copy(&mut input, &mut e).and_then(|n| e.finish().and_then(|mut o| o.flush()).map(|_| n))
    };
    match r {
        Ok(n) => written += n,
        Err(e) => {
            if !to_stdout {
                let _ = std::fs::remove_file(&out_name);
            }
            return Err(format!("{f}: {}", err(&e)));
        }
    }
    if verbose && f != "-" {
        let (a, b) = if decompress { (in_size, written) } else { (written, in_size) };
        let pct = if b > 0 { 100.0 - a as f64 * 100.0 / b as f64 } else { 0.0 };
        eprintln!("{f}:\t{pct:.1}% -- {} {out_name}", if to_stdout { "written to stdout," } else { "replaced with" });
    }
    if !to_stdout && !keep && f != "-" {
        std::fs::remove_file(f).map_err(|e| format!("{f}: {}", err(&e)))?;
    }
    let _ = name;
    Ok(())
}
