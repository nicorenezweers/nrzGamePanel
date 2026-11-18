//! Build- und Release-Werkzeuge für nrzGamePanel.
//!
//! ```text
//! cargo xtask umbrel-release 1.0.1 "Kurze Beschreibung"
//! ```
//!
//! Baut nrzGamePanel für umbrelOS (Linux amd64 + arm64) und die Weboberfläche, legt die fertigen
//! Dateien in das öffentliche Paket-Repo nrzGamePanel-umbrel (Zweig `umbrel`) und pusht den Tag
//! `umbrel-<version>`. nrzGit trägt Paket und Version per nrzStore.json selbst im nrzStore ein.
//!
//! Voraussetzungen: dioxus-cli 0.7 (dx), zig + cargo-zigbuild, Rust-Targets
//! x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu und wasm32-unknown-unknown.
//! Umgebung: `DX`, `NRZGP_PKG_REMOTE`, `NRZGP_BUILD_DIR`, `ALLOW_DIRTY=1`, `KEEP_BUILD=1`.

mod pre_start;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("umbrel-release") => umbrel_release(&args[1..]),
        _ => Err("Aufruf: cargo xtask umbrel-release <version> [notizen]".into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fehler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn umbrel_release(args: &[String]) -> Result {
    let version = args.first().ok_or("Version fehlt, z.B. 1.0.1")?.clone();
    let notes = args.get(1).cloned().unwrap_or_else(|| format!("nrzGamePanel {version}"));
    let tag = format!("umbrel-{version}");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    env::set_current_dir(&root).map_err(|e| e.to_string())?;

    let pkg_remote = match env::var("NRZGP_PKG_REMOTE") {
        Ok(r) => r,
        Err(_) => output(Command::new("git").args(["remote", "get-url", "origin"]))?
            .trim()
            .replace("/nrzGamePanel.git", "/nrzGamePanel-umbrel.git"),
    };

    if !output(Command::new("git").args(["status", "--porcelain"]))?.trim().is_empty()
        && env::var("ALLOW_DIRTY").as_deref() != Ok("1")
    {
        return Err("Arbeitsverzeichnis nicht sauber - erst committen.".into());
    }

    let dx = find_dx().ok_or("dioxus-cli (dx) nicht gefunden - cargo binstall dioxus-cli")?;
    if !quiet(Command::new("cargo").args(["zigbuild", "--help"])) {
        return Err("cargo-zigbuild fehlt".into());
    }

    // Pfade ohne Leerzeichen (zig-Wrapper und C-Builds wie ring vertragen keine).
    let build = PathBuf::from(env::var("NRZGP_BUILD_DIR").unwrap_or_else(|_| "/tmp/nrzgp-build".into()));
    let out = build.join("out");
    let _ = fs::remove_dir_all(&out);
    mkdir(&out)?;
    mkdir(&build.join("bin"))?;
    let ar = build.join("bin/zig-ar");
    let ranlib = build.join("bin/zig-ranlib");
    write_exec(&ar, "#!/bin/sh\nexec zig ar \"$@\"\n")?;
    write_exec(&ranlib, "#!/bin/sh\nexec zig ranlib \"$@\"\n")?;

    println!("==> Oberflaeche (Dioxus/WASM)");
    // Alte Assets frueherer Builds nicht mitliefern.
    let webdist = build.join("webdist");
    let _ = fs::remove_dir_all(&webdist);
    let _ = fs::remove_dir_all(build.join("wasm/dx/nrz-gamepanel-web/release/web/public"));
    run(Command::new(&dx)
        .current_dir(root.join("crates/web"))
        .env("CARGO_TARGET_DIR", build.join("wasm"))
        .args(["bundle", "--platform", "web", "--release", "--out-dir"])
        .arg(&webdist))?;
    let public = webdist.join("public");
    if !public.join("index.html").is_file() {
        return Err("Web-Bundle ohne index.html".into());
    }
    run(Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .args(["--no-xattrs", "-czf"])
        .arg(out.join("web.tar.gz"))
        .arg("-C")
        .arg(&public)
        .arg("."))?;

    for (target, arch) in [("x86_64-unknown-linux-gnu", "amd64"), ("aarch64-unknown-linux-gnu", "arm64")] {
        println!("==> Panel {arch} ({target})");
        let linux = build.join("linux");
        run(Command::new("cargo")
            .env("CARGO_TARGET_DIR", &linux)
            .env("CARGO_ZIGBUILD_CACHE_DIR", build.join("zigcache"))
            .env("AR", &ar)
            .env("RANLIB", &ranlib)
            .args(["zigbuild", "--release", "-p", "nrz-gamepanel", "--target", &format!("{target}.2.31")]))?;
        let bin = linux.join(target).join("release/nrz-gamepanel");
        let gz = fs::File::create(out.join(format!("nrz-gamepanel-{arch}.gz"))).map_err(|e| e.to_string())?;
        run(Command::new("gzip").args(["-9", "-c"]).arg(&bin).stdout(gz))?;
        // Speicher sparen: Zwischenstaende dieses Targets wegwerfen
        if env::var("KEEP_BUILD").as_deref() != Ok("1") {
            let _ = fs::remove_dir_all(linux.join(target).join("release/deps"));
            let _ = fs::remove_dir_all(linux.join(target).join("release/build"));
        }
    }
    let sums = output(
        Command::new("shasum")
            .current_dir(&out)
            .args(["-a", "256", "nrz-gamepanel-amd64.gz", "nrz-gamepanel-arm64.gz", "web.tar.gz"]),
    )?;
    fs::write(out.join("SHA256SUMS"), sums).map_err(|e| e.to_string())?;

    println!("==> Paket-Repo {pkg_remote}");
    let wt = TempDir::new(&build.join("pkg"))?;
    let wtp = wt.0.as_path();
    if quiet(Command::new("git").args(["ls-remote", "--exit-code", &pkg_remote, "umbrel"])) {
        run(Command::new("git").args(["clone", "-q", "--depth", "1", "--branch", "umbrel", &pkg_remote]).arg(wtp))?;
    } else {
        run(Command::new("git").args(["init", "-q", "-b", "umbrel"]).arg(wtp))?;
        run(git(wtp).args(["remote", "add", "origin", &pkg_remote]))?;
    }
    if quiet(git(wtp).args(["ls-remote", "--exit-code", "--tags", "origin", &tag])) {
        return Err(format!("Tag {tag} gibt es schon."));
    }
    for e in fs::read_dir(wtp).map_err(|e| e.to_string())?.flatten() {
        if e.file_name() != ".git" {
            remove(&e.path())?;
        }
    }
    for e in fs::read_dir(&out).map_err(|e| e.to_string())?.flatten() {
        copy(&e.path(), &wtp.join(e.file_name()))?;
    }
    copy(&root.join("scripts/umbrel-README.md"), &wtp.join("README.md"))?;
    copy(&root.join("scripts/nrzStore.json"), &wtp.join("nrzStore.json"))?;
    let pkg = wtp.join("package");
    copy(&root.join("umbrel"), &pkg)?;
    // Version und RELEASE im Paket auf diesen Stand setzen
    let manifest = pkg.join("umbrel-app.yml");
    let yml = fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
    let yml: String = yml
        .lines()
        .map(|l| if l.starts_with("version: \"") { format!("version: \"{version}\"\n") } else { format!("{l}\n") })
        .collect();
    fs::write(&manifest, yml).map_err(|e| e.to_string())?;
    mkdir(&pkg.join("hooks"))?;
    write_exec(&pkg.join("hooks/pre-start"), &pre_start::PRE_START.replace("{RELEASE}", &tag))?;

    run(git(wtp).args(["add", "-A"]))?;
    run(git(wtp).args(["commit", "-qm", &format!("nrzGamePanel {version}: Umbrel-Paket"), "-m", &notes]))?;
    run(git(wtp).args(["tag", "-a", &tag, "-m", &notes]))?;
    run(git(wtp).args(["push", "-q", "origin", "umbrel"]))?;
    run(git(wtp).args(["push", "-q", "origin", &tag]))?;
    println!("==> {tag} veroeffentlicht. nrzGit aktualisiert den nrzStore (Protokoll: GET /api/v1/workflow).");
    Ok(())
}

fn find_dx() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = env::var_os("DX").map(PathBuf::from).into_iter().collect();
    if let Some(path) = env::var_os("PATH") {
        cands.extend(env::split_paths(&path).map(|d| d.join("dx")));
    }
    if let Some(home) = env::var_os("HOME") {
        cands.push(Path::new(&home).join(".cargo/bin/dx"));
    }
    cands.into_iter().filter(|c| c.is_file()).find(|c| {
        Command::new(c)
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("dioxus"))
    })
}

fn git(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir);
    c
}

fn run(cmd: &mut Command) -> Result {
    let st = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("{cmd:?} fehlgeschlagen ({st})")) }
}

fn output(cmd: &mut Command) -> Result<String> {
    let o = cmd.stderr(Stdio::inherit()).output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !o.status.success() {
        return Err(format!("{cmd:?} fehlgeschlagen ({})", o.status));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn quiet(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn mkdir(p: &Path) -> Result {
    fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn remove(p: &Path) -> Result {
    let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
    r.map_err(|e| format!("{}: {e}", p.display()))
}

/// Kopiert Datei oder Verzeichnis rekursiv (Rechte bleiben erhalten).
fn copy(from: &Path, to: &Path) -> Result {
    if from.is_dir() {
        mkdir(to)?;
        for e in fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
            copy(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ()).map_err(|e| format!("{}: {e}", from.display()))
    }
}

fn write_exec(p: &Path, content: &str) -> Result {
    fs::write(p, content).map_err(|e| format!("{}: {e}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Arbeitsverzeichnis, das beim Verlassen wieder entfernt wird.
struct TempDir(PathBuf);

impl TempDir {
    fn new(p: &Path) -> Result<Self> {
        let _ = fs::remove_dir_all(p);
        if let Some(parent) = p.parent() {
            mkdir(parent)?;
        }
        Ok(Self(p.to_path_buf()))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
