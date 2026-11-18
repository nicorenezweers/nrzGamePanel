//! Baut das CSS der Oberfläche aus den Rust-Quellen in `style/` mit dem Tailwind-CLI
//! (dieselbe Binärdatei, die dioxus-cli unter `~/.dx/tools` ablegt; sonst `TAILWINDCSS`
//! oder `tailwindcss` im PATH). Ohne Tailwind wird das mitgelieferte `style/bundle.rs` genutzt.
//! `NRZ_CSS_UPDATE=1` schreibt das frisch gebaute CSS nach `style/bundle.rs` zurück.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

mod nrzui {
    include!("style/nrzui.rs");
}
mod panel {
    include!("style/panel.rs");
}
mod bundle {
    include!("style/bundle.rs");
}

fn main() {
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=style");
    println!("cargo::rerun-if-env-changed=TAILWINDCSS");
    println!("cargo::rerun-if-env-changed=NRZ_CSS_UPDATE");

    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = out.join("nrz.css");
    let release = env::var("PROFILE").as_deref() == Ok("release");
    let update = env::var("NRZ_CSS_UPDATE").is_ok_and(|v| v == "1");

    let src = crate_dir.join("src").display().to_string().replace('\\', "/");
    let input = out.join("input.css");
    fs::write(
        &input,
        format!(
            "@import \"tailwindcss\" source(none);\n@source \"{src}/**/*.rs\";\n{}\n{}\n{}",
            nrzui::THEME,
            nrzui::APP,
            panel::PANEL
        ),
    )
    .unwrap();

    let css = tailwind().and_then(|tw| {
        let raw = compile(&tw, &input, &out.join("raw.css"), false)?;
        if update {
            write_bundle(&crate_dir.join("style/bundle.rs"), &raw);
        }
        if release { compile(&tw, &input, &out.join("min.css"), true) } else { Some(raw) }
    });
    let css = css.unwrap_or_else(|| {
        println!("cargo::warning=Tailwind nicht gefunden – nutze style/bundle.rs");
        bundle::CSS.trim_start().to_string()
    });
    fs::write(target, css).unwrap();
}

fn compile(tw: &Path, input: &Path, output: &Path, minify: bool) -> Option<String> {
    let mut cmd = Command::new(tw);
    cmd.arg("-i").arg(input).arg("-o").arg(output);
    if minify {
        cmd.arg("--minify");
    }
    match cmd.output() {
        Ok(o) if o.status.success() => fs::read_to_string(output).ok(),
        Ok(o) => {
            for line in String::from_utf8_lossy(&o.stderr).lines() {
                println!("cargo::warning=tailwindcss: {line}");
            }
            None
        }
        Err(_) => None,
    }
}

fn tailwind() -> Option<PathBuf> {
    if let Some(p) = env::var_os("TAILWINDCSS") {
        return Some(PathBuf::from(p));
    }
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let p = dir.join("tailwindcss");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))?;
    let mut found: Vec<PathBuf> = fs::read_dir(Path::new(&home).join(".dx/tools"))
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("tailwindcss-v4"))
        .map(|e| e.path().join("tailwindcss"))
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found.pop()
}

fn write_bundle(path: &Path, css: &str) {
    assert!(!css.contains("\"#"), "CSS enthält \"# – Rohstring anpassen");
    let body = format!(
        "// Fertig gebautes CSS (Tailwind + style/nrzui.rs + style/panel.rs) für Builds ohne Tailwind-CLI.\n\
         // Nicht von Hand ändern: `NRZ_CSS_UPDATE=1 cargo check -p nrz-gamepanel-web --target wasm32-unknown-unknown`.\n\
         pub const CSS: &str = r#\"\n{}\"#;\n",
        css.trim_end_matches('\n').to_string() + "\n"
    );
    if fs::read_to_string(path).ok().as_deref() != Some(body.as_str()) {
        fs::write(path, body).unwrap();
    }
}
