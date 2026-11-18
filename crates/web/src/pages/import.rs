//! Server importieren: automatische Suche mit Ein-Klick-Import, Ordner-Browser und Analyse-Ansicht.

use dioxus::prelude::*;
use nrz_gp_common::{
    AnalyzeRequest, Confidence, Flavor, Game, HostDirListing, ImportAnalysis, ImportMode, ImportRequest, LaunchKind, PortCheck, ScanRequest,
    ServerSummary, routes::enc,
};

use crate::{
    Route, api,
    components::{
        CARD, CheckRow, ConfirmDialog, ErrorBox, Field, INPUT, Loading, Modal, Notice, SELECT, Segmented, Tag,
        ui::{
            app_shell::{EmptyState, PageContainer, PageHeader, PageWidth},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    store::{SYSTEM, refresh_servers, toast, toast_err},
};

#[derive(Clone, PartialEq)]
enum View {
    Scan,
    Browse,
    Analysis(ImportAnalysis),
}

const MC_FLAVORS: &[Flavor] = &[
    Flavor::Paper,
    Flavor::Purpur,
    Flavor::Folia,
    Flavor::Spigot,
    Flavor::Bukkit,
    Flavor::Vanilla,
    Flavor::Fabric,
    Flavor::Quilt,
    Flavor::Forge,
    Flavor::NeoForge,
    Flavor::Unknown,
];

fn confidence_tag(c: Confidence) -> Element {
    match c {
        Confidence::High => rsx! { Tag { tone: "success", icons::ShieldCheck { class: "size-3" } "Sicher erkannt" } },
        Confidence::Medium => rsx! { Tag { tone: "warning", "Wahrscheinlich" } },
        Confidence::Low => rsx! { Tag { tone: "destructive", "Unsicher" } },
    }
}

fn software_line(a: &ImportAnalysis) -> String {
    let game = a.game.map_or("Unbekanntes Spiel", Game::label);
    let mut s = game.to_string();
    if a.game == Some(Game::Minecraft) {
        s.push_str(" · ");
        s.push_str(a.flavor.label());
    }
    if let Some(v) = &a.version {
        s.push(' ');
        s.push_str(v);
    }
    s
}

fn addon_line(a: &ImportAnalysis) -> Option<String> {
    match (a.plugin_count, a.mod_count) {
        (0, 0) => None,
        (p, 0) => Some(format!("{p} Plugin{}", if p == 1 { "" } else { "s" })),
        (0, m) => Some(format!("{m} Mod{}", if m == 1 { "" } else { "s" })),
        (p, m) => Some(format!("{p} Plugins, {m} Mods")),
    }
}

/// Braucht der Import eine EULA-Bestätigung?
fn needs_eula(a: &ImportAnalysis) -> bool {
    a.game != Some(Game::Factorio) && a.eula_accepted != Some(true)
}

/// Ein-Klick-Import mit den erkannten Werten.
async fn quick_import(a: ImportAnalysis, accept_eula: bool) -> Result<ServerSummary, String> {
    let req = ImportRequest {
        path: a.path.clone(),
        name: a.suggested_name.clone(),
        mode: ImportMode::InPlace,
        enable_rcon: true,
        accept_eula,
        ..Default::default()
    };
    api::post::<_, ServerSummary>("/api/import", &req).await
}

#[component]
pub fn ImportServer() -> Element {
    let nav = navigator();
    let mut view = use_signal(|| View::Scan);
    let mut scan = use_resource(|| async move { api::post::<_, Vec<ImportAnalysis>>("/api/import/scan", &ScanRequest { root: None }).await });
    let mut importing = use_signal(|| None::<String>);
    let mut eula_for = use_signal(|| None::<ImportAnalysis>);

    let mut start_import = move |a: ImportAnalysis, accept: bool| {
        importing.set(Some(a.path.clone()));
        spawn(async move {
            match quick_import(a, accept).await {
                Ok(s) => {
                    refresh_servers().await;
                    toast(format!("„{}“ importiert", s.config.name));
                    nav.push(Route::ServerOverview { id: s.config.id });
                }
                Err(e) => {
                    toast_err(e);
                    importing.set(None);
                }
            }
        });
    };

    let v = view();
    let tab = if matches!(v, View::Browse) { "browse" } else { "scan" };

    rsx! {
        PageContainer { width: PageWidth::Default,
            PageHeader {
                title: "Server importieren",
                description: "Bestehende Minecraft- oder Factorio-Server übernehmen – Welten, Plugins und Einstellungen bleiben, wie sie sind.",
            }
            match v {
                View::Analysis(a) => rsx! {
                    AnalysisView { key: "{a.path}", analysis: a, onback: move |_| view.set(View::Scan) }
                },
                _ => rsx! {
                    div { class: "mb-5 flex flex-wrap items-center justify-between gap-3",
                        Segmented {
                            items: vec![("scan".into(), "Gefundene Server".into()), ("browse".into(), "Ordner wählen".into())],
                            active: tab,
                            onchange: move |t: String| view.set(if t == "browse" { View::Browse } else { View::Scan }),
                        }
                        if tab == "scan" && scan.read().is_some() {
                            Button { size: ButtonSize::Sm, variant: ButtonVariant::Ghost, onclick: move |_| scan.restart(),
                                icons::RefreshCw { class: "size-4" }
                                "Erneut suchen"
                            }
                        }
                    }
                    if tab == "browse" {
                        Browser { onanalyzed: move |a| view.set(View::Analysis(a)) }
                    } else {
                        match &*scan.read() {
                            None => rsx! { Loading { text: "Suche bestehende Server …" } },
                            Some(Err(e)) => rsx! {
                                ErrorBox { message: format!("Suche fehlgeschlagen: {e}") }
                            },
                            Some(Ok(list)) if list.is_empty() => rsx! {
                                NothingFound { onbrowse: move |_| view.set(View::Browse) }
                            },
                            Some(Ok(list)) => rsx! {
                                p { class: "mb-3 text-sm text-muted-foreground",
                                    if list.len() == 1 { "1 Server gefunden." } else { "{list.len()} Server gefunden." }
                                    " Ein Klick auf „Importieren“ übernimmt die erkannten Werte – anpassen kannst du alles auch später."
                                }
                                div { class: "grid gap-3 md:grid-cols-2",
                                    for a in list.iter().cloned() {
                                        ScanCard {
                                            key: "{a.path}",
                                            busy: importing().as_deref() == Some(a.path.as_str()),
                                            disabled: importing().is_some(),
                                            analysis: a.clone(),
                                            onimport: {
                                                let a = a.clone();
                                                move |_| {
                                                    if needs_eula(&a) {
                                                        eula_for.set(Some(a.clone()));
                                                    } else {
                                                        start_import(a.clone(), false);
                                                    }
                                                }
                                            },
                                            onedit: {
                                                let a = a.clone();
                                                move |_| view.set(View::Analysis(a.clone()))
                                            },
                                        }
                                    }
                                }
                            },
                        }
                    }
                },
            }
        }
        if let Some(a) = eula_for() {
            EulaDialog {
                name: a.suggested_name.clone(),
                onanswer: move |accept: bool| {
                    eula_for.set(None);
                    start_import(a.clone(), accept);
                },
                oncancel: move |_| eula_for.set(None),
            }
        }
    }
}

/// Rückfrage zur Minecraft-EULA vor dem Import.
#[component]
fn EulaDialog(name: String, onanswer: EventHandler<bool>, oncancel: EventHandler<()>) -> Element {
    rsx! {
        Modal {
            title: "Minecraft-EULA akzeptieren?",
            onclose: move |_| oncancel.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Ghost, onclick: move |_| oncancel.call(()), "Abbrechen" }
                Button { variant: ButtonVariant::Outline, onclick: move |_| onanswer.call(false), "Ohne EULA importieren" }
                Button { onclick: move |_| onanswer.call(true), "Akzeptieren und importieren" }
            },
            div { class: "flex flex-col gap-3 text-sm",
                p {
                    "Für „{name}“ ist die "
                    a { class: "underline", href: "https://aka.ms/MinecraftEULA", target: "_blank", rel: "noopener", "Minecraft-EULA" }
                    " noch nicht akzeptiert. Ohne Zustimmung startet der Server nicht."
                }
                p { class: "text-muted-foreground", "Du kannst die EULA auch später in der Server-Übersicht akzeptieren." }
            }
        }
    }
}

/// Karte eines gefundenen Servers.
#[component]
fn ScanCard(analysis: ImportAnalysis, busy: bool, disabled: bool, onimport: EventHandler<()>, onedit: EventHandler<()>) -> Element {
    let a = analysis;
    let done = a.already_imported.clone();
    let dim = if done.is_some() { "opacity-60" } else { "" };
    rsx! {
        div { class: "{CARD} flex flex-col gap-3 p-4 {dim}",
            div { class: "flex items-start gap-3",
                div { class: "flex size-10 shrink-0 items-center justify-center rounded-lg bg-muted",
                    if a.game == Some(Game::Factorio) {
                        icons::Factory { class: "size-5" }
                    } else {
                        icons::Pickaxe { class: "size-5" }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        span { class: "truncate font-medium", "{a.suggested_name}" }
                        if done.is_some() {
                            Tag { "Bereits importiert" }
                        } else {
                            {confidence_tag(a.confidence)}
                        }
                    }
                    p { class: "text-sm text-muted-foreground", "{software_line(&a)}" }
                    p { class: "mt-0.5 truncate font-mono text-xs text-muted-foreground", title: "{a.path}", "{a.path}" }
                }
            }
            div { class: "flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground",
                if !a.worlds.is_empty() {
                    span { class: "inline-flex items-center gap-1",
                        icons::Earth { class: "size-3.5" }
                        "{a.worlds.join(\", \")}"
                    }
                }
                if let Some(l) = addon_line(&a) {
                    span { class: "inline-flex items-center gap-1",
                        icons::Puzzle { class: "size-3.5" }
                        "{l}"
                    }
                }
                if let Some(p) = a.port {
                    span { class: "inline-flex items-center gap-1",
                        icons::Network { class: "size-3.5" }
                        "Port {p}"
                    }
                }
                if needs_eula(&a) {
                    span { class: "inline-flex items-center gap-1 text-warning",
                        icons::TriangleAlert { class: "size-3.5" }
                        "EULA offen"
                    }
                }
            }
            div { class: "mt-auto flex flex-wrap items-center justify-end gap-2 pt-1",
                if let Some(id) = done {
                    Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, href: format!("/servers/{id}"),
                        "Server öffnen"
                    }
                } else {
                    Button { size: ButtonSize::Sm, variant: ButtonVariant::Ghost, disabled, onclick: move |_| onedit.call(()), "Anpassen" }
                    Button { size: ButtonSize::Sm, disabled, onclick: move |_| onimport.call(()),
                        if busy {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                            "Importiere …"
                        } else {
                            icons::FolderInput { class: "size-4" }
                            "Importieren"
                        }
                    }
                }
            }
        }
    }
}

/// Leerzustand: erklärt, welche Ordner das Panel sieht.
#[component]
fn NothingFound(onbrowse: EventHandler<()>) -> Element {
    let roots = SYSTEM.read().as_ref().map(|s| s.import_roots.clone()).unwrap_or_default();
    rsx! {
        EmptyState {
            title: "Keine Server gefunden",
            description: "Das Panel hat in seinen Import-Ordnern keinen Minecraft- oder Factorio-Server entdeckt.",
            icon: rsx! { icons::FolderSearch {} },
            div { class: "flex w-full max-w-md flex-col gap-3 text-left text-sm",
                if roots.is_empty() {
                    p { class: "text-muted-foreground", "Es sind keine Import-Ordner eingerichtet." }
                } else {
                    div {
                        p { class: "mb-1.5 text-muted-foreground", "Durchsucht werden:" }
                        div { class: "flex flex-wrap gap-1.5",
                            for r in roots {
                                code { key: "{r}", class: "rounded bg-muted px-1.5 py-0.5 font-mono text-xs", "{r}" }
                            }
                        }
                    }
                }
                p { class: "text-muted-foreground",
                    "Liegt dein Server woanders, binde den Ordner per Docker-Volume ein (z. B. "
                    code { class: "font-mono text-xs", "-v /pfad/zum/server:/import/server" }
                    ") oder ergänze ihn in "
                    code { class: "font-mono text-xs", "NRZ_GP_IMPORT_ROOTS" }
                    "."
                }
                div { class: "flex justify-center pt-1",
                    Button { variant: ButtonVariant::Outline, onclick: move |_| onbrowse.call(()),
                        icons::FolderOpen { class: "size-4" }
                        "Ordner selbst wählen"
                    }
                }
            }
        }
    }
}

/// Ordner-Browser auf dem Host.
#[component]
fn Browser(onanalyzed: EventHandler<ImportAnalysis>) -> Element {
    let mut path = use_signal(|| None::<String>);
    let mut typed = use_signal(String::new);
    let mut analyzing = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let listing = use_resource(move || async move {
        let p = path();
        let url = match &p {
            Some(p) => format!("/api/fs/browse?path={}", enc(p)),
            None => "/api/fs/browse".to_string(),
        };
        api::get::<HostDirListing>(&url).await
    });
    // Eingabefeld folgt dem aktuellen Ordner
    use_effect(move || {
        if let Some(Ok(l)) = &*listing.read() {
            typed.set(l.path.clone());
        }
    });

    let mut analyze = move |p: String| {
        analyzing.set(Some(p.clone()));
        error.set(None);
        spawn(async move {
            match api::post::<_, ImportAnalysis>("/api/import/analyze", &AnalyzeRequest { path: p }).await {
                Ok(a) => onanalyzed.call(a),
                Err(e) => error.set(Some(e)),
            }
            analyzing.set(None);
        });
    };

    let data = listing.read().clone();
    let cur_path = match &data {
        Some(Ok(l)) => l.path.clone(),
        _ => path().unwrap_or_default(),
    };
    let crumbs: Vec<(String, String)> = {
        let mut acc = String::new();
        let mut v = vec![("/".to_string(), "/".to_string())];
        for part in cur_path.split('/').filter(|s| !s.is_empty()) {
            acc.push('/');
            acc.push_str(part);
            v.push((part.to_string(), acc.clone()));
        }
        v
    };

    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "{CARD} flex flex-col gap-3 p-4",
                if let Some(Ok(l)) = &data {
                    if !l.roots.is_empty() {
                        div { class: "flex flex-wrap items-center gap-1.5",
                            span { class: "mr-1 text-xs text-muted-foreground", "Startpunkte" }
                            for r in l.roots.iter().cloned() {
                                button {
                                    key: "{r}",
                                    r#type: "button",
                                    class: if cur_path == r { "rounded-full bg-primary px-2.5 py-0.5 font-mono text-xs text-primary-foreground" } else { "rounded-full border px-2.5 py-0.5 font-mono text-xs hover:bg-accent" },
                                    onclick: move |_| path.set(Some(r.clone())),
                                    "{r}"
                                }
                            }
                        }
                    }
                }
                form {
                    class: "flex gap-2",
                    onsubmit: move |e: FormEvent| {
                        e.prevent_default();
                        let t = typed().trim().to_string();
                        if !t.is_empty() {
                            path.set(Some(t));
                        }
                    },
                    input {
                        class: "{INPUT} font-mono",
                        value: "{typed}",
                        placeholder: "/pfad/zum/server",
                        spellcheck: false,
                        oninput: move |e| typed.set(e.value()),
                    }
                    Button { button_type: "submit", variant: ButtonVariant::Outline, "Öffnen" }
                }
                nav { class: "flex flex-wrap items-center gap-0.5 text-sm", aria_label: "Pfad",
                    for (i , (label , p)) in crumbs.iter().cloned().enumerate() {
                        span { key: "{p}", class: "inline-flex items-center gap-0.5",
                            if i > 1 {
                                icons::ChevronRight { class: "size-3.5 text-muted-foreground" }
                            }
                            button {
                                r#type: "button",
                                class: "rounded px-1.5 py-0.5 font-mono text-xs hover:bg-accent",
                                onclick: move |_| path.set(Some(p.clone())),
                                "{label}"
                            }
                        }
                    }
                }
            }

            if let Some(e) = error() {
                ErrorBox { message: format!("Analyse fehlgeschlagen: {e}") }
            }

            div { class: "{CARD} overflow-hidden",
                match &data {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! { div { class: "p-4", ErrorBox { message: e.clone() } } },
                    Some(Ok(l)) => rsx! {
                        div { class: "flex flex-wrap items-center justify-between gap-2 border-b px-4 py-3",
                            div { class: "min-w-0",
                                p { class: "truncate font-mono text-sm", "{l.path}" }
                                p { class: "text-xs text-muted-foreground", "Wähle den Ordner, in dem die Server-Dateien liegen." }
                            }
                            Button {
                                size: ButtonSize::Sm,
                                disabled: analyzing().is_some(),
                                onclick: {
                                    let p = l.path.clone();
                                    move |_| analyze(p.clone())
                                },
                                if analyzing().as_deref() == Some(l.path.as_str()) {
                                    icons::LoaderCircle { class: "size-4 animate-spin" }
                                    "Analysiere …"
                                } else {
                                    icons::ScanSearch { class: "size-4" }
                                    "Diesen Ordner analysieren"
                                }
                            }
                        }
                        div { class: "max-h-[28rem] divide-y overflow-y-auto",
                            if let Some(parent) = l.parent.clone() {
                                button {
                                    r#type: "button",
                                    class: "flex w-full items-center gap-2.5 px-4 py-2 text-left text-sm text-muted-foreground hover:bg-accent/40",
                                    onclick: move |_| path.set(Some(parent.clone())),
                                    icons::CornerLeftUp { class: "size-4" }
                                    ".."
                                }
                            }
                            if l.dirs.is_empty() {
                                p { class: "px-4 py-8 text-center text-sm text-muted-foreground", "Keine Unterordner." }
                            }
                            for d in l.dirs.iter().cloned() {
                                div {
                                    key: "{d.path}",
                                    class: if d.looks_like_server { "flex items-center gap-2 bg-success/5 pr-2" } else { "flex items-center gap-2 pr-2" },
                                    button {
                                        r#type: "button",
                                        class: "flex min-w-0 flex-1 items-center gap-2.5 px-4 py-2 text-left text-sm hover:bg-accent/40",
                                        onclick: {
                                            let p = d.path.clone();
                                            move |_| path.set(Some(p.clone()))
                                        },
                                        if d.looks_like_server {
                                            icons::Server { class: "size-4 shrink-0 text-success" }
                                        } else {
                                            icons::Folder { class: "size-4 shrink-0 text-muted-foreground" }
                                        }
                                        span { class: "truncate", "{d.name}" }
                                        if d.looks_like_server {
                                            Tag { tone: "success", "Server erkannt" }
                                        }
                                    }
                                    if d.looks_like_server {
                                        Button {
                                            size: ButtonSize::Sm,
                                            variant: ButtonVariant::Outline,
                                            disabled: analyzing().is_some(),
                                            onclick: {
                                                let p = d.path.clone();
                                                move |_| analyze(p.clone())
                                            },
                                            if analyzing().as_deref() == Some(d.path.as_str()) {
                                                icons::LoaderCircle { class: "size-4 animate-spin" }
                                            }
                                            "Analysieren"
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

fn launch_icon(l: &LaunchKind) -> Element {
    match l {
        LaunchKind::Jar { .. } => rsx! { icons::Coffee { class: "size-4" } },
        LaunchKind::ArgsFile { .. } => rsx! { icons::FileCode { class: "size-4" } },
        LaunchKind::Script { .. } => rsx! { icons::SquareTerminal { class: "size-4" } },
        LaunchKind::Factorio { .. } => rsx! { icons::Factory { class: "size-4" } },
    }
}

/// Analyse-Ansicht mit allen erkannten Werten und editierbaren Feldern.
#[component]
fn AnalysisView(analysis: ImportAnalysis, onback: EventHandler<()>) -> Element {
    let nav = navigator();
    let a = analysis.clone();
    let initial_cand = a.candidates.iter().position(|c| Some(&c.launch) == a.launch.as_ref()).unwrap_or(0);
    let mut cand = use_signal(|| initial_cand);
    let mut name = use_signal(|| a.suggested_name.clone());
    let mut version = use_signal(|| a.version.clone().unwrap_or_default());
    let mut flavor = use_signal(|| a.flavor);
    let mut memory = use_signal(|| a.memory_mb.unwrap_or(4096).to_string());
    let mut port = use_signal(|| a.port.or_else(|| a.game.map(Game::default_port)).map(|p| p.to_string()).unwrap_or_default());
    let mut java = use_signal(String::new); // leer = automatisch
    let mut mode = use_signal(|| ImportMode::InPlace);
    let mut rcon = use_signal(|| true);
    let mut eula = use_signal(|| false);
    let mut auto_start = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut confirm_no_eula = use_signal(|| false);

    let game_q = if a.game == Some(Game::Factorio) { "factorio" } else { "minecraft" };
    let port_check = use_resource(move || async move {
        let n = port().trim().parse::<u16>().ok().filter(|n| *n > 0)?;
        gloo_timers::future::TimeoutFuture::new(300).await;
        api::get::<PortCheck>(&format!("/api/check-port?port={n}&game={game_q}")).await.ok()
    });

    let is_mc = a.game != Some(Game::Factorio);
    let need_eula = needs_eula(&a);
    let javas = SYSTEM.read().as_ref().map(|s| s.java.clone()).unwrap_or_default();
    let port_num = port().trim().parse::<u16>().ok().filter(|p| *p > 0);
    let port_state = port_check.read().clone().flatten();
    let mem_num = memory().trim().parse::<u32>().ok().filter(|m| *m >= 256);
    let valid = !name().trim().is_empty() && port_num.is_some() && (!is_mc || mem_num.is_some()) && a.already_imported.is_none();

    let do_import = {
        let a = a.clone();
        move |accept_eula: bool| {
            let a = a.clone();
            busy.set(true);
            error.set(None);
            let launch = a.candidates.get(cand()).map(|c| c.launch.clone()).or_else(|| a.launch.clone());
            let v = version().trim().to_string();
            let j = java();
            let req = ImportRequest {
                path: a.path.clone(),
                name: name().trim().to_string(),
                mode: mode(),
                game: a.game,
                flavor: Some(flavor()),
                version: (!v.is_empty()).then_some(v),
                launch,
                java_path: (!j.is_empty()).then_some(j),
                memory_mb: memory().trim().parse().ok(),
                port: port().trim().parse().ok(),
                enable_rcon: rcon(),
                accept_eula,
                auto_start: auto_start(),
            };
            spawn(async move {
                match api::post::<_, ServerSummary>("/api/import", &req).await {
                    Ok(s) => {
                        refresh_servers().await;
                        toast(format!("„{}“ importiert", s.config.name));
                        nav.push(Route::ServerOverview { id: s.config.id });
                    }
                    Err(e) => {
                        error.set(Some(e));
                        busy.set(false);
                    }
                }
            });
        }
    };
    let mut do_import2 = do_import.clone();
    let mut do_import3 = do_import.clone();

    let java_label = |path: &str| javas.iter().find(|j| j.path == path).map(|j| format!("Java {} ({})", j.major, j.version)).unwrap_or_else(|| path.to_string());

    rsx! {
        div { class: "flex flex-col gap-5",
            div {
                Button { size: ButtonSize::Sm, variant: ButtonVariant::Ghost, onclick: move |_| onback.call(()),
                    icons::ChevronLeft { class: "size-4" }
                    "Zurück zur Auswahl"
                }
            }

            // Kopf
            div { class: "{CARD} flex flex-col gap-4 p-4 sm:p-5",
                div { class: "flex items-start gap-3",
                    div { class: "flex size-11 shrink-0 items-center justify-center rounded-lg bg-muted",
                        if a.game == Some(Game::Factorio) {
                            icons::Factory { class: "size-5" }
                        } else {
                            icons::Pickaxe { class: "size-5" }
                        }
                    }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex flex-wrap items-center gap-2",
                            h2 { class: "text-lg font-semibold", "{a.suggested_name}" }
                            {confidence_tag(a.confidence)}
                        }
                        p { class: "text-sm text-muted-foreground", "{software_line(&a)}" }
                        p { class: "mt-0.5 truncate font-mono text-xs text-muted-foreground", title: "{a.path}", "{a.path}" }
                    }
                }
                dl { class: "grid grid-cols-1 gap-x-6 gap-y-2 text-sm sm:grid-cols-2",
                    Detected { label: "Spiel", value: a.game.map_or("unbekannt", Game::label) }
                    if is_mc {
                        Detected { label: "Software", value: a.flavor.label() }
                    }
                    Detected {
                        label: "Version",
                        value: a.version.clone().unwrap_or_else(|| "unbekannt".into()),
                        source: a.version_source.clone(),
                    }
                    if let Some(l) = &a.loader_version {
                        Detected { label: "Loader", value: l.clone() }
                    }
                    if let Some(j) = a.java_required {
                        Detected { label: "Java benötigt", value: format!("Java {j}") }
                    }
                    if is_mc {
                        Detected {
                            label: "Java gewählt",
                            value: a.java_path.as_deref().map(java_label).unwrap_or_else(|| "wird beim Start geladen".into()),
                        }
                        Detected { label: "Arbeitsspeicher", value: a.memory_mb.map_or("nicht angegeben".into(), |m| format!("{m} MB")) }
                    }
                    Detected { label: "Port", value: a.port.map_or("Standard".into(), |p| p.to_string()) }
                    Detected { label: "Welten", value: if a.worlds.is_empty() { "keine gefunden".into() } else { a.worlds.join(", ") } }
                    if let Some(l) = addon_line(&a) {
                        Detected { label: "Erweiterungen", value: l }
                    }
                    if is_mc {
                        Detected {
                            label: "EULA",
                            value: match a.eula_accepted {
                                Some(true) => "akzeptiert",
                                Some(false) => "nicht akzeptiert",
                                None => "fehlt",
                            },
                        }
                    }
                    Detected { label: "RCON", value: if a.has_rcon { "bereits eingerichtet" } else { "nicht eingerichtet" } }
                    if let Some(jvm) = a.jvm_args.as_ref().filter(|s| !s.trim().is_empty()) {
                        Detected { label: "JVM-Argumente", value: jvm.clone() }
                    }
                }
            }

            if let Some(id) = a.already_imported.clone() {
                Notice {
                    warning: true,
                    action: rsx! {
                        Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, href: format!("/servers/{id}"), "Server öffnen" }
                    },
                    "Dieser Ordner ist bereits als Server im Panel."
                }
            }
            for (i , n) in a.notes.iter().enumerate() {
                Notice { key: "{i}", "{n}" }
            }

            // Startdatei
            div { class: "{CARD} flex flex-col gap-3 p-4 sm:p-5",
                div {
                    h3 { class: "font-semibold", "Startdatei" }
                    p { class: "text-sm text-muted-foreground", "So startet das Panel den Server. Die beste Möglichkeit ist vorausgewählt." }
                }
                if a.candidates.is_empty() {
                    Notice { warning: true, "Keine Startdatei gefunden. Du kannst sie nach dem Import in den Einstellungen festlegen." }
                }
                div { class: "flex flex-col gap-2",
                    for (i , c) in a.candidates.iter().cloned().enumerate() {
                        label {
                            key: "{i}",
                            class: if cand() == i { "flex cursor-pointer items-start gap-3 rounded-lg border border-foreground/40 bg-accent/40 p-3" } else { "flex cursor-pointer items-start gap-3 rounded-lg border p-3 hover:bg-accent/30" },
                            input {
                                r#type: "radio",
                                name: "candidate",
                                class: "mt-1 accent-primary",
                                checked: cand() == i,
                                onchange: move |_| {
                                    cand.set(i);
                                    if c.flavor != Flavor::Unknown {
                                        flavor.set(c.flavor);
                                    }
                                },
                            }
                            div { class: "min-w-0 flex-1",
                                div { class: "flex flex-wrap items-center gap-2",
                                    span { class: "text-muted-foreground", {launch_icon(&c.launch)} }
                                    code { class: "font-mono text-sm break-all", "{c.launch.describe()}" }
                                    if i == 0 {
                                        Tag { tone: "success", "Beste Wahl" }
                                    }
                                }
                                p { class: "mt-0.5 text-xs text-muted-foreground",
                                    if c.flavor != Flavor::Unknown {
                                        "{c.flavor.label()} · "
                                    }
                                    "{c.reason} · Wertung {c.score}"
                                }
                            }
                        }
                    }
                }
            }

            // Einstellungen
            div { class: "{CARD} flex flex-col gap-4 p-4 sm:p-5",
                h3 { class: "font-semibold", "Übernehmen als" }
                div { class: "grid gap-4 sm:grid-cols-2",
                    Field { label: "Name", class: "sm:col-span-2",
                        input { class: INPUT, value: "{name}", oninput: move |e| name.set(e.value()) }
                    }
                    if is_mc {
                        Field { label: "Software",
                            select {
                                class: SELECT,
                                onchange: move |e| {
                                    if let Some(f) = MC_FLAVORS.iter().find(|f| f.label() == e.value()) {
                                        flavor.set(*f);
                                    }
                                },
                                for f in MC_FLAVORS.iter() {
                                    option { key: "{f.label()}", value: f.label(), selected: flavor() == *f, "{f.label()}" }
                                }
                            }
                        }
                    }
                    Field { label: "Version", hint: "Leer lassen, wenn unbekannt.",
                        input { class: INPUT, value: "{version}", placeholder: "z. B. 1.21.4", oninput: move |e| version.set(e.value()) }
                    }
                    if is_mc {
                        Field { label: "Arbeitsspeicher (MB)", hint: "z. B. 4096 = 4 GB",
                            input {
                                class: INPUT,
                                inputmode: "numeric",
                                value: "{memory}",
                                oninput: move |e| memory.set(e.value().chars().filter(char::is_ascii_digit).take(6).collect()),
                            }
                        }
                    }
                    Field { label: "Port",
                        div { class: "flex items-center gap-2",
                            input {
                                class: "{INPUT} font-mono",
                                inputmode: "numeric",
                                value: "{port}",
                                oninput: move |e| port.set(e.value().chars().filter(char::is_ascii_digit).take(5).collect()),
                            }
                            if let Some(c) = port_state.as_ref().filter(|c| Some(c.port) == port_num) {
                                if c.free {
                                    span { class: "shrink-0 text-xs text-success", "frei" }
                                } else {
                                    span { class: "shrink-0 text-xs text-warning",
                                        match &c.used_by {
                                            Some(n) => format!("belegt ({n})"),
                                            None => "belegt".to_string(),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if is_mc {
                        Field { label: "Java", hint: "Automatisch wählt passend zur Version (fehlende lädt das Panel herunter).",
                            select { class: SELECT, onchange: move |e| java.set(e.value()),
                                option { value: "", selected: java().is_empty(), "Automatisch" }
                                for j in javas.iter() {
                                    option { key: "{j.path}", value: "{j.path}", selected: java() == j.path,
                                        "Java {j.major} ({j.version}) – {j.path}"
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "flex flex-col gap-2",
                    span { class: "text-sm font-medium", "Ordner" }
                    div { class: "grid gap-2 sm:grid-cols-2",
                        ModeCard {
                            selected: mode() == ImportMode::InPlace,
                            title: "An Ort und Stelle verwenden",
                            text: "Empfohlen. Die Dateien bleiben, wo sie sind, und werden beim Löschen nie entfernt.",
                            onclick: move |_| mode.set(ImportMode::InPlace),
                        }
                        ModeCard {
                            selected: mode() == ImportMode::Copy,
                            title: "In das Panel kopieren",
                            text: "Erstellt eine Kopie im Datenordner des Panels. Das Original bleibt unverändert.",
                            onclick: move |_| mode.set(ImportMode::Copy),
                        }
                    }
                }

                div { class: "flex flex-col gap-3 border-t pt-4",
                    CheckRow {
                        checked: rcon(),
                        onchange: move |v| rcon.set(v),
                        label: "RCON automatisch einrichten",
                        hint: "Empfohlen – nötig für Live-Daten wie Spielerliste, Positionen und Inventare.",
                    }
                    if is_mc && need_eula {
                        CheckRow {
                            checked: eula(),
                            onchange: move |v| eula.set(v),
                            label: "Minecraft-EULA akzeptieren",
                            a {
                                class: "w-fit text-xs underline text-muted-foreground hover:text-foreground",
                                href: "https://aka.ms/MinecraftEULA",
                                target: "_blank",
                                rel: "noopener",
                                "EULA lesen"
                            }
                        }
                    }
                    CheckRow { checked: auto_start(), onchange: move |v| auto_start.set(v), label: "Automatisch starten", hint: "Nach dem Import und wenn das Panel startet." }
                }

                if let Some(e) = error() {
                    ErrorBox { message: e }
                }

                div { class: "flex flex-wrap items-center justify-end gap-2 border-t pt-4",
                    Button { variant: ButtonVariant::Ghost, onclick: move |_| onback.call(()), "Abbrechen" }
                    Button {
                        disabled: !valid || busy(),
                        onclick: move |_| {
                            if is_mc && need_eula && !eula() {
                                confirm_no_eula.set(true);
                            } else {
                                do_import2(is_mc && need_eula && eula());
                            }
                        },
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                            "Importiere …"
                        } else {
                            icons::FolderInput { class: "size-4" }
                            "Importieren"
                        }
                    }
                }
            }
        }
        if confirm_no_eula() {
            ConfirmDialog {
                title: "Ohne EULA importieren?",
                message: "Die Minecraft-EULA ist nicht akzeptiert – der Server startet erst, wenn du sie akzeptierst (geht auch später in der Übersicht).",
                confirm_label: "Trotzdem importieren",
                onconfirm: move |_| {
                    confirm_no_eula.set(false);
                    do_import3(false);
                },
                oncancel: move |_| confirm_no_eula.set(false),
            }
        }
    }
}

#[component]
fn Detected(#[props(into)] label: String, #[props(into)] value: String, #[props(optional)] source: Option<String>) -> Element {
    rsx! {
        div { class: "flex min-w-0 items-baseline justify-between gap-3 border-b border-dashed pb-1.5 sm:justify-start",
            dt { class: "w-32 shrink-0 text-muted-foreground", "{label}" }
            dd { class: "min-w-0 text-right break-words sm:text-left",
                span { class: "font-medium", "{value}" }
                if let Some(s) = source {
                    span { class: "ml-1.5 text-xs text-muted-foreground", "aus {s}" }
                }
            }
        }
    }
}

#[component]
fn ModeCard(selected: bool, #[props(into)] title: String, #[props(into)] text: String, onclick: EventHandler<MouseEvent>) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if selected { "flex items-start gap-2.5 rounded-lg border border-foreground/40 bg-accent/40 p-3 text-left" } else { "flex items-start gap-2.5 rounded-lg border p-3 text-left hover:bg-accent/30" },
            onclick: move |e| onclick.call(e),
            span { class: if selected { "mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border-4 border-primary" } else { "mt-0.5 size-4 shrink-0 rounded-full border" } }
            span { class: "flex flex-col gap-0.5",
                span { class: "text-sm font-medium", "{title}" }
                span { class: "text-xs text-muted-foreground", "{text}" }
            }
        }
    }
}
