//! Datei-Explorer: Ordner durchsuchen, hochladen (auch per Drag & Drop), Textdateien bearbeiten,
//! umbenennen, löschen, herunterladen, entpacken und Archive erstellen.

use std::collections::HashSet;

use dioxus::prelude::*;
use nrz_gp_common::{DirListing, FileContent, FileEntry, Flavor, Game, PathRequest, PathsRequest, RenameRequest, WriteFile, routes};
use serde::Serialize;

use crate::{
    api,
    components::{
        ConfirmDialog, Dropdown, ErrorBox, ICON_BUTTON, INPUT, Loading, MENU_ITEM, MENU_ITEM_DANGER, MenuSeparator, Modal, Toggle,
        drag_has_files, dropped_files, fmt_ago, fmt_bytes, fmt_datetime, open_file_picker, take_input_files,
        ui::button::{Button, ButtonSize, ButtonVariant},
    },
    layout::use_server,
    store::{toast, toast_err},
};

const UPLOAD_INPUT: &str = "files-upload-input";

#[derive(Clone, PartialEq)]
enum Dialog {
    None,
    NewFolder,
    NewFile,
    Rename(FileEntry),
    Delete(Vec<String>),
    Binary { path: String, size: u64 },
}

#[derive(Clone, Copy, PartialEq)]
enum SortBy {
    Name,
    Size,
    Modified,
}

/// Geöffnete Textdatei.
#[derive(Clone, PartialEq)]
struct OpenFile {
    path: String,
    content: String,
    size: u64,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.trim_start_matches('/').to_string() } else { format!("{dir}/{}", name.trim_start_matches('/')) }
}

fn parent(path: &str) -> String {
    path.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_archive(name: &str) -> bool {
    let n = name.to_lowercase();
    [".zip", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar"].iter().any(|e| n.ends_with(e))
}

fn download_url(id: &str, path: &str) -> String {
    format!("{}?path={}", routes::server(id, "files/download"), routes::enc(path))
}

fn entry_icon(e: &FileEntry) -> Element {
    if e.is_dir {
        return rsx! { icons::Folder { class: "size-4 shrink-0 text-muted-foreground" } };
    }
    let n = e.name.to_lowercase();
    let ext = n.rsplit('.').next().unwrap_or("");
    let cls = "size-4 shrink-0 text-muted-foreground";
    if is_archive(&n) || ext == "jar" {
        return rsx! { icons::FileArchive { class: cls } };
    }
    match ext {
        "log" | "txt" | "md" => rsx! { icons::FileText { class: cls } },
        "json" | "yml" | "yaml" | "toml" | "properties" | "cfg" | "conf" | "ini" | "sh" | "bat" | "js" | "mcmeta" => {
            rsx! { icons::FileCode { class: cls } }
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => rsx! { icons::FileImage { class: cls } },
        _ => rsx! { icons::File { class: cls } },
    }
}

async fn file_op<B: Serialize>(id: &str, op: &str, body: &B) -> Result<(), String> {
    api::post::<_, nrz_gp_common::Ok>(&routes::server(id, &format!("files/{op}")), body).await.map(|_| ())
}

/// Schnellzugriffe je nach Spiel und Software: `(Pfad, Ordner?)`.
fn quick_targets(game: Game, flavor: Flavor) -> Vec<(&'static str, bool)> {
    match game {
        Game::Minecraft => {
            let mut v = vec![("server.properties", false), ("eula.txt", false), ("logs/latest.log", false)];
            if flavor.uses_mods() {
                v.push(("mods", true));
                v.push(("config", true));
            } else if flavor != Flavor::Vanilla {
                v.push(("plugins", true));
                v.push(("config", true));
            }
            v
        }
        Game::Factorio => vec![
            ("factorio-current.log", false),
            ("server-settings.json", false),
            ("data/server-settings.json", false),
            ("saves", true),
            ("mods", true),
        ],
    }
}

#[component]
pub fn ServerFiles(id: String) -> Element {
    let ctx = use_server();
    let mut path = use_signal(String::new);
    let mut reload = use_signal(|| 0u32);
    let mut selected = use_signal(HashSet::<String>::new);
    let mut dialog = use_signal(|| Dialog::None);
    let mut editor = use_signal(|| None::<OpenFile>);
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| (SortBy::Name, true));
    let mut drag_over = use_signal(|| false);
    let mut uploading = use_signal(|| 0usize);
    let mut opening = use_signal(|| false);

    let listing = use_resource(move || {
        let id = ctx.id();
        let p = path();
        let _ = reload();
        async move { api::get::<DirListing>(&format!("{}?path={}", routes::server(&id, "files"), routes::enc(&p))).await }
    });
    let root = use_resource(move || {
        let id = ctx.id();
        let _ = reload();
        async move { api::get::<DirListing>(&format!("{}?path=", routes::server(&id, "files"))).await.ok() }
    });

    let mut go = move |p: String| {
        path.set(p);
        selected.write().clear();
        filter.set(String::new());
    };

    let mut open_path = move |p: String| {
        if opening() {
            return;
        }
        opening.set(true);
        let id = ctx.id();
        spawn(async move {
            match api::get::<FileContent>(&format!("{}?path={}", routes::server(&id, "files/content"), routes::enc(&p))).await {
                Ok(c) if c.binary => dialog.set(Dialog::Binary { path: p, size: c.size }),
                Ok(c) => editor.set(Some(OpenFile { path: p, content: c.content, size: c.size })),
                Err(e) => toast_err(e),
            }
            opening.set(false);
        });
    };

    let mut do_upload = move |files: Vec<web_sys::File>| {
        if files.is_empty() {
            return;
        }
        let n = files.len();
        let id = ctx.id();
        let p = path();
        uploading.set(n);
        spawn(async move {
            let url = format!("{}?path={}", routes::server(&id, "files/upload"), routes::enc(&p));
            match api::upload(&url, &files).await {
                Ok(()) => toast(if n == 1 { "Datei hochgeladen".to_string() } else { format!("{n} Dateien hochgeladen") }),
                Err(e) => toast_err(format!("Hochladen fehlgeschlagen: {e}")),
            }
            uploading.set(0);
            reload += 1;
        });
    };

    // Einfache Operation mit Meldung und Neuladen.
    let run_op = move |op: &'static str, body: serde_json::Value, ok_msg: String| {
        let id = ctx.id();
        spawn(async move {
            match file_op(&id, op, &body).await {
                Ok(()) => toast(ok_msg),
                Err(e) => toast_err(e),
            }
            selected.write().clear();
            reload += 1;
        });
    };

    let (game, flavor) = ctx.summary.read().as_ref().map_or((Game::Minecraft, Flavor::Vanilla), |s| (s.config.game, s.config.flavor));
    let root_names: HashSet<String> = match &*root.read() {
        Some(Some(l)) => l.entries.iter().map(|e| e.name.clone()).collect(),
        _ => HashSet::new(),
    };
    let quick: Vec<(&'static str, bool)> = quick_targets(game, flavor)
        .into_iter()
        .filter(|(p, _)| root_names.contains(p.split('/').next().unwrap_or(p)))
        .collect();

    let cur = path();
    let segments: Vec<(String, String)> = {
        let mut acc = String::new();
        cur.split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                acc = join(&acc, s);
                (s.to_string(), acc.clone())
            })
            .collect()
    };

    let (sort_by, asc) = sort();
    let f = filter().to_lowercase();
    let listing_state = listing.read().clone();
    let entries: Vec<FileEntry> = match &listing_state {
        Some(Ok(l)) => {
            let mut v: Vec<FileEntry> = l.entries.iter().filter(|e| f.is_empty() || e.name.to_lowercase().contains(&f)).cloned().collect();
            v.sort_by(|a, b| {
                b.is_dir.cmp(&a.is_dir).then_with(|| {
                    let o = match sort_by {
                        SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                        SortBy::Size => a.size.cmp(&b.size),
                        SortBy::Modified => a.modified.cmp(&b.modified),
                    };
                    if asc { o } else { o.reverse() }
                })
            });
            v
        }
        _ => Vec::new(),
    };
    let total = match &listing_state {
        Some(Ok(l)) => l.entries.len(),
        _ => 0,
    };
    let sel = selected.read().clone();
    let all_selected = !entries.is_empty() && entries.iter().all(|e| sel.contains(&e.path));
    let sel_list: Vec<String> = sel.iter().cloned().collect();
    let single_sel: Option<FileEntry> = if sel.len() == 1 { entries.iter().find(|e| sel.contains(&e.path)).cloned() } else { None };
    let id_dl = id.clone();

    let sort_header = move |label: &'static str, by: SortBy, extra: &'static str| {
        let active = sort_by == by;
        rsx! {
            button {
                class: "inline-flex items-center gap-1 hover:text-foreground {extra}",
                onclick: move |_| {
                    let (b, a) = sort();
                    sort.set(if b == by { (by, !a) } else { (by, by == SortBy::Name) });
                },
                "{label}"
                if active {
                    if asc {
                        icons::ArrowUp { class: "size-3" }
                    } else {
                        icons::ArrowDown { class: "size-3" }
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-4 p-4 md:p-8",
            // Schnellzugriffe
            if !quick.is_empty() {
                div { class: "flex flex-wrap items-center gap-2",
                    span { class: "mr-1 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Schnellzugriff" }
                    for (p, is_dir) in quick {
                        button {
                            key: "{p}",
                            class: "inline-flex items-center gap-1.5 rounded-md border bg-card px-2.5 py-1 text-xs shadow-xs hover:bg-accent",
                            onclick: move |_| if is_dir { go(p.to_string()) } else { open_path(p.to_string()) },
                            if is_dir {
                                icons::Folder { class: "size-3.5 text-muted-foreground" }
                            } else {
                                icons::FileText { class: "size-3.5 text-muted-foreground" }
                            }
                            "{p}"
                        }
                    }
                }
            }

            // Pfad + Werkzeuge
            div { class: "flex flex-wrap items-center justify-between gap-3",
                nav { class: "flex min-w-0 flex-wrap items-center gap-0.5 text-sm", aria_label: "Pfad",
                    button {
                        class: if segments.is_empty() { "inline-flex items-center gap-1.5 rounded-md px-2 py-1 font-medium" } else { "inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-muted-foreground hover:bg-accent hover:text-foreground" },
                        onclick: move |_| go(String::new()),
                        icons::House { class: "size-4" }
                        "Server"
                    }
                    for (i, (name, full)) in segments.iter().cloned().enumerate() {
                        icons::ChevronRight { key: "c{i}", class: "size-3.5 text-muted-foreground" }
                        if i + 1 == segments.len() {
                            span { key: "s{i}", class: "rounded-md px-2 py-1 font-medium", "{name}" }
                        } else {
                            button {
                                key: "s{i}",
                                class: "rounded-md px-2 py-1 text-muted-foreground hover:bg-accent hover:text-foreground",
                                onclick: move |_| go(full.clone()),
                                "{name}"
                            }
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    div { class: "relative",
                        icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                        input {
                            class: "{INPUT} h-8 w-40 pl-8 sm:w-48",
                            placeholder: "In Ordner suchen",
                            value: "{filter}",
                            oninput: move |e| filter.set(e.value()),
                        }
                    }
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| dialog.set(Dialog::NewFolder),
                        icons::FolderPlus { class: "size-4" }
                        span { class: "hidden sm:inline", "Ordner" }
                    }
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| dialog.set(Dialog::NewFile),
                        icons::FilePlus { class: "size-4" }
                        span { class: "hidden sm:inline", "Datei" }
                    }
                    Button {
                        size: ButtonSize::Sm,
                        disabled: uploading() > 0,
                        onclick: move |_| open_file_picker(UPLOAD_INPUT),
                        if uploading() > 0 {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::Upload { class: "size-4" }
                        }
                        "Hochladen"
                    }
                    input {
                        id: UPLOAD_INPUT,
                        class: "hidden",
                        r#type: "file",
                        multiple: true,
                        onchange: move |_| do_upload(take_input_files(UPLOAD_INPUT)),
                    }
                    button {
                        class: ICON_BUTTON,
                        title: "Aktualisieren",
                        aria_label: "Aktualisieren",
                        onclick: move |_| reload += 1,
                        icons::RefreshCw { class: "size-4" }
                    }
                }
            }

            // Auswahl-Leiste
            if !sel.is_empty() {
                div { class: "flex flex-wrap items-center gap-2 rounded-lg border bg-muted/50 px-3 py-2 text-sm animate-nrz-enter",
                    span { class: "font-medium", "{sel.len()} ausgewählt" }
                    div { class: "flex-1" }
                    if let Some(e) = single_sel.clone() {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            onclick: {
                                let id = id_dl.clone();
                                move |_| api::download(&download_url(&id, &e.path))
                            },
                            icons::Download { class: "size-4" }
                            "Herunterladen"
                        }
                    }
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: {
                            let paths = sel_list.clone();
                            move |_| run_op("archive", serde_json::json!(PathsRequest { paths: paths.clone() }), "Archiv erstellt".into())
                        },
                        icons::Archive { class: "size-4" }
                        "Archiv erstellen"
                    }
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Destructive,
                        onclick: {
                            let paths = sel_list.clone();
                            move |_| dialog.set(Dialog::Delete(paths.clone()))
                        },
                        icons::Trash2 { class: "size-4" }
                        "Löschen"
                    }
                    button {
                        class: ICON_BUTTON,
                        title: "Auswahl aufheben",
                        aria_label: "Auswahl aufheben",
                        onclick: move |_| selected.write().clear(),
                        icons::X { class: "size-4" }
                    }
                }
            }

            // Liste (auch Ablagefläche für Drag & Drop)
            div {
                class: "relative rounded-xl border bg-card shadow-sm",
                ondragenter: move |e| {
                    if drag_has_files(&e) {
                        e.prevent_default();
                        drag_over.set(true);
                    }
                },
                ondragover: move |e| {
                    if drag_has_files(&e) {
                        e.prevent_default();
                        if !drag_over() {
                            drag_over.set(true);
                        }
                    }
                },
                ondrop: move |e| {
                    e.prevent_default();
                    drag_over.set(false);
                    do_upload(dropped_files(&e));
                },
                if drag_over() {
                    div {
                        class: "absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed border-foreground/40 bg-background/85 text-sm",
                        ondragover: move |e| e.prevent_default(),
                        ondragleave: move |_| drag_over.set(false),
                        ondrop: move |e| {
                            e.prevent_default();
                            e.stop_propagation();
                            drag_over.set(false);
                            do_upload(dropped_files(&e));
                        },
                        icons::Upload { class: "size-6 text-muted-foreground pointer-events-none" }
                        span { class: "pointer-events-none font-medium",
                            if cur.is_empty() { "Loslassen zum Hochladen in den Server-Ordner" } else { "Loslassen zum Hochladen nach {cur}" }
                        }
                    }
                }
                // Kopfzeile
                div { class: "grid grid-cols-[auto_1fr_auto] items-center gap-3 border-b px-3 py-2 text-xs font-medium text-muted-foreground sm:grid-cols-[auto_1fr_6rem_9rem_auto]",
                    input {
                        r#type: "checkbox",
                        class: "size-4 accent-primary",
                        aria_label: "Alle auswählen",
                        checked: all_selected,
                        disabled: entries.is_empty(),
                        onchange: {
                            let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
                            move |_| {
                                if all_selected {
                                    selected.write().clear();
                                } else {
                                    selected.set(paths.iter().cloned().collect());
                                }
                            }
                        },
                    }
                    {sort_header("Name", SortBy::Name, "")}
                    {sort_header("Grösse", SortBy::Size, "hidden justify-end sm:inline-flex")}
                    {sort_header("Geändert", SortBy::Modified, "hidden sm:inline-flex")}
                    span { class: "w-8" }
                }
                if !cur.is_empty() {
                    button {
                        class: "grid w-full grid-cols-[auto_1fr] items-center gap-3 border-b px-3 py-2 text-left text-sm text-muted-foreground hover:bg-accent/40",
                        onclick: {
                            let p = parent(&cur);
                            move |_| go(p.clone())
                        },
                        span { class: "flex size-4 items-center justify-center", icons::CornerLeftUp { class: "size-4" } }
                        span { ".. (übergeordneter Ordner)" }
                    }
                }
                match &listing_state {
                    None => rsx! { Loading {} },
                    Some(Err(e)) => rsx! {
                        div { class: "flex flex-col items-start gap-3 p-4",
                            ErrorBox { message: e.clone() }
                            Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, onclick: move |_| reload += 1, "Erneut versuchen" }
                        }
                    },
                    Some(Ok(_)) if entries.is_empty() => rsx! {
                        div { class: "flex flex-col items-center gap-2 px-4 py-12 text-center",
                            div { class: "grid size-12 place-items-center rounded-xl bg-muted text-muted-foreground",
                                if total > 0 { icons::Search { class: "size-6" } } else { icons::FolderOpen { class: "size-6" } }
                            }
                            p { class: "font-medium", if total > 0 { "Keine Treffer" } else { "Ordner ist leer" } }
                            p { class: "max-w-sm text-sm text-muted-foreground",
                                if total > 0 { "Keine Datei in diesem Ordner passt zu „{filter}“." } else { "Ziehe Dateien hierher oder nutze „Hochladen“, um etwas abzulegen." }
                            }
                        }
                    },
                    Some(Ok(_)) => rsx! {
                        for e in entries.iter().cloned() {
                            FileRow {
                                key: "{e.path}",
                                server_id: id.clone(),
                                checked: sel.contains(&e.path),
                                entry: e.clone(),
                                ontoggle: {
                                    let p = e.path.clone();
                                    move |on: bool| {
                                        if on {
                                            selected.write().insert(p.clone());
                                        } else {
                                            selected.write().remove(&p);
                                        }
                                    }
                                },
                                onopen: {
                                    let e = e.clone();
                                    move |_| if e.is_dir { go(e.path.clone()) } else { open_path(e.path.clone()) }
                                },
                                onrename: {
                                    let e = e.clone();
                                    move |_| dialog.set(Dialog::Rename(e.clone()))
                                },
                                ondelete: {
                                    let p = e.path.clone();
                                    move |_| dialog.set(Dialog::Delete(vec![p.clone()]))
                                },
                                onextract: {
                                    let p = e.path.clone();
                                    move |_| run_op("extract", serde_json::json!(PathRequest { path: p.clone() }), "Archiv entpackt".into())
                                },
                                onarchive: {
                                    let p = e.path.clone();
                                    move |_| run_op("archive", serde_json::json!(PathsRequest { paths: vec![p.clone()] }), "Archiv erstellt".into())
                                },
                            }
                        }
                    },
                }
                if opening() {
                    div { class: "absolute inset-0 z-10 flex items-center justify-center rounded-xl bg-background/50",
                        Loading { text: "Öffne Datei …" }
                    }
                }
            }
            p { class: "text-xs text-muted-foreground",
                "Tipp: Dateien direkt auf die Liste ziehen, um sie hochzuladen. Ordner werden beim Herunterladen als ZIP gepackt."
            }
        }

        // Dialoge
        match dialog() {
            Dialog::None => rsx! {},
            Dialog::NewFolder => rsx! {
                NameDialog {
                    title: "Neuer Ordner",
                    label: "Ordnername",
                    initial: "",
                    confirm_label: "Erstellen",
                    onclose: move |_| dialog.set(Dialog::None),
                    onsubmit: move |name: String| {
                        dialog.set(Dialog::None);
                        run_op("mkdir", serde_json::json!(PathRequest { path: join(&path(), &name) }), format!("Ordner „{name}“ erstellt"));
                    },
                }
            },
            Dialog::NewFile => rsx! {
                NameDialog {
                    title: "Neue Datei",
                    label: "Dateiname",
                    initial: "",
                    confirm_label: "Erstellen",
                    onclose: move |_| dialog.set(Dialog::None),
                    onsubmit: move |name: String| {
                        dialog.set(Dialog::None);
                        let p = join(&path(), &name);
                        let id = ctx.id();
                        spawn(async move {
                            match file_op(&id, "touch", &PathRequest { path: p.clone() }).await {
                                Ok(()) => {
                                    reload += 1;
                                    editor.set(Some(OpenFile { path: p, content: String::new(), size: 0 }));
                                }
                                Err(e) => toast_err(e),
                            }
                        });
                    },
                }
            },
            Dialog::Rename(entry) => rsx! {
                NameDialog {
                    title: if entry.is_dir { "Ordner umbenennen" } else { "Datei umbenennen" },
                    label: "Neuer Name",
                    initial: entry.name.clone(),
                    confirm_label: "Umbenennen",
                    hint: "Mit „/“ lässt sich auch in einen Unterordner verschieben.",
                    onclose: move |_| dialog.set(Dialog::None),
                    onsubmit: move |name: String| {
                        dialog.set(Dialog::None);
                        if name != entry.name {
                            let to = join(&parent(&entry.path), &name);
                            run_op("rename", serde_json::json!(RenameRequest { from: entry.path.clone(), to }), "Umbenannt".into());
                        }
                    },
                }
            },
            Dialog::Delete(paths) => {
                let msg = if paths.len() == 1 {
                    format!("„{}“ wird endgültig gelöscht. Ordner samt Inhalt.", file_name(&paths[0]))
                } else {
                    format!("{} Einträge werden endgültig gelöscht (Ordner samt Inhalt).", paths.len())
                };
                rsx! {
                    ConfirmDialog {
                        title: "Löschen?",
                        message: msg,
                        confirm_label: "Löschen",
                        destructive: true,
                        oncancel: move |_| dialog.set(Dialog::None),
                        onconfirm: move |_| {
                            dialog.set(Dialog::None);
                            let n = paths.len();
                            run_op(
                                "delete",
                                serde_json::json!(PathsRequest { paths: paths.clone() }),
                                if n == 1 { "Gelöscht".into() } else { format!("{n} Einträge gelöscht") },
                            );
                        },
                    }
                }
            }
            Dialog::Binary { path: p, size } => rsx! {
                Modal {
                    title: "Datei kann nicht angezeigt werden",
                    onclose: move |_| dialog.set(Dialog::None),
                    footer: rsx! {
                        Button { variant: ButtonVariant::Outline, onclick: move |_| dialog.set(Dialog::None), "Schliessen" }
                        Button {
                            onclick: {
                                let url = download_url(&id, &p);
                                move |_| {
                                    api::download(&url);
                                    dialog.set(Dialog::None);
                                }
                            },
                            icons::Download { class: "size-4" }
                            "Herunterladen"
                        }
                    },
                    p { class: "text-sm text-muted-foreground",
                        "„{file_name(&p)}“ ({fmt_bytes(size)}) ist eine Binärdatei oder zu gross für den Editor. Du kannst sie herunterladen."
                    }
                }
            },
        }

        if let Some(f) = editor() {
            FileEditor {
                key: "{f.path}",
                server_id: ctx.id(),
                path: f.path.clone(),
                initial: f.content.clone(),
                size: f.size,
                onclose: move |changed: bool| {
                    editor.set(None);
                    if changed {
                        reload += 1;
                    }
                },
            }
        }
    }
}

#[component]
fn FileRow(
    server_id: String,
    entry: FileEntry,
    checked: bool,
    ontoggle: EventHandler<bool>,
    onopen: EventHandler<()>,
    onrename: EventHandler<()>,
    ondelete: EventHandler<()>,
    onextract: EventHandler<()>,
    onarchive: EventHandler<()>,
) -> Element {
    let row_bg = if checked { "bg-accent/50" } else { "hover:bg-accent/40" };
    let archive = !entry.is_dir && is_archive(&entry.name);
    let dl = download_url(&server_id, &entry.path);
    let size = if entry.is_dir { "–".to_string() } else { fmt_bytes(entry.size) };
    rsx! {
        div { class: "grid grid-cols-[auto_1fr_auto] items-center gap-3 border-b px-3 py-1.5 text-sm last:border-b-0 sm:grid-cols-[auto_1fr_6rem_9rem_auto] {row_bg}",
            input {
                r#type: "checkbox",
                class: "size-4 accent-primary",
                aria_label: "{entry.name} auswählen",
                checked,
                onchange: move |e| ontoggle.call(e.checked()),
            }
            button {
                class: "flex min-w-0 items-center gap-2.5 py-0.5 text-left",
                title: if entry.is_dir { "Ordner öffnen" } else { "Datei öffnen" },
                onclick: move |_| onopen.call(()),
                {entry_icon(&entry)}
                span { class: "min-w-0",
                    span { class: "block truncate hover:underline", "{entry.name}" }
                    span { class: "block text-xs text-muted-foreground sm:hidden",
                        if entry.is_dir { "Ordner · {fmt_ago(entry.modified)}" } else { "{size} · {fmt_ago(entry.modified)}" }
                    }
                }
            }
            span { class: "hidden text-right tabular-nums text-muted-foreground sm:block", "{size}" }
            span { class: "hidden text-muted-foreground sm:block", title: "{fmt_ago(entry.modified)}", "{fmt_datetime(entry.modified)}" }
            Dropdown { label: "Aktionen für {entry.name}",
                button { class: MENU_ITEM, onclick: move |_| onopen.call(()),
                    if entry.is_dir {
                        icons::FolderOpen {}
                        "Öffnen"
                    } else {
                        icons::Pencil {}
                        "Öffnen / Bearbeiten"
                    }
                }
                button { class: MENU_ITEM, onclick: move |_| api::download(&dl),
                    icons::Download {}
                    if entry.is_dir { "Als ZIP herunterladen" } else { "Herunterladen" }
                }
                button { class: MENU_ITEM, onclick: move |_| onrename.call(()),
                    icons::PenLine {}
                    "Umbenennen"
                }
                if archive {
                    button { class: MENU_ITEM, onclick: move |_| onextract.call(()),
                        icons::PackageOpen {}
                        "Hier entpacken"
                    }
                }
                button { class: MENU_ITEM, onclick: move |_| onarchive.call(()),
                    icons::Archive {}
                    "Als ZIP archivieren"
                }
                MenuSeparator {}
                button { class: MENU_ITEM_DANGER, onclick: move |_| ondelete.call(()),
                    icons::Trash2 {}
                    "Löschen"
                }
            }
        }
    }
}

/// Dialog mit einem Namensfeld (Neu, Umbenennen). Enter bestätigt, Esc schliesst.
#[component]
fn NameDialog(
    #[props(into)] title: String,
    #[props(into)] label: String,
    #[props(into)] initial: String,
    #[props(into)] confirm_label: String,
    #[props(into, optional)] hint: Option<String>,
    onsubmit: EventHandler<String>,
    onclose: EventHandler<()>,
) -> Element {
    let mut name = use_signal(|| initial.clone());
    let valid = {
        let n = name();
        let t = n.trim();
        !t.is_empty() && t != "." && t != ".." && !t.contains('\\')
    };
    let submit = move || {
        let n = name().trim().trim_matches('/').to_string();
        if !n.is_empty() {
            onsubmit.call(n);
        }
    };
    let submit2 = submit;
    rsx! {
        Modal {
            title,
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button { disabled: !valid, onclick: move |_| submit2(), "{confirm_label}" }
            },
            form {
                onsubmit: move |e| {
                    e.prevent_default();
                    if valid {
                        submit();
                    }
                },
                label { class: "flex flex-col gap-1.5",
                    span { class: "text-sm font-medium", "{label}" }
                    input {
                        class: INPUT,
                        value: "{name}",
                        autofocus: true,
                        spellcheck: "false",
                        autocomplete: "off",
                        onmounted: move |e| {
                            spawn(async move {
                                let _ = e.data().set_focus(true).await;
                            });
                        },
                        oninput: move |e| name.set(e.value()),
                    }
                    if let Some(h) = hint {
                        span { class: "text-xs text-muted-foreground", "{h}" }
                    }
                }
            }
        }
    }
}

/// Texteditor (Vollbild-Dialog). Strg/Cmd+S speichert, Esc schliesst (mit Rückfrage bei Änderungen).
#[component]
fn FileEditor(server_id: String, path: String, initial: String, size: u64, onclose: EventHandler<bool>) -> Element {
    let mut content = use_signal(|| initial.clone());
    let mut saved = use_signal(|| initial.clone());
    let mut wrap = use_signal(|| path.ends_with(".txt") || path.ends_with(".md") || path.ends_with(".log"));
    let mut saving = use_signal(|| false);
    let mut confirm_close = use_signal(|| false);
    let mut ever_saved = use_signal(|| false);
    let dirty = use_memo(move || *content.read() != *saved.read());
    let sid = server_id.clone();
    let p = path.clone();

    let save = use_callback(move |()| {
        if saving() || !dirty() {
            return;
        }
        saving.set(true);
        let id = sid.clone();
        let path = p.clone();
        let c = content();
        spawn(async move {
            match api::put::<_, nrz_gp_common::Ok>(&routes::server(&id, "files/content"), &WriteFile { path, content: c.clone() }).await {
                Ok(_) => {
                    saved.set(c);
                    ever_saved.set(true);
                    toast("Gespeichert");
                }
                Err(e) => toast_err(format!("Speichern fehlgeschlagen: {e}")),
            }
            saving.set(false);
        });
    });
    let request_close = use_callback(move |()| {
        if dirty() {
            confirm_close.set(true);
        } else {
            onclose.call(ever_saved());
        }
    });

    let lines = content.read().lines().count().max(1);
    let bytes = content.read().len() as u64;
    let name = file_name(&path).to_string();
    let wrap_cls = if wrap() { "whitespace-pre-wrap break-all" } else { "whitespace-pre" };

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex bg-black/60 animate-nrz-fade md:p-6",
            onkeydown: move |e: KeyboardEvent| {
                let m = e.modifiers();
                let is_s = matches!(e.key(), Key::Character(ref c) if c.eq_ignore_ascii_case("s"));
                if is_s && (m.contains(Modifiers::CONTROL) || m.contains(Modifiers::META)) {
                    e.prevent_default();
                    save.call(());
                } else if e.key() == Key::Escape && !confirm_close() {
                    e.stop_propagation();
                    request_close.call(());
                }
            },
            div { class: "flex min-h-0 w-full flex-col overflow-hidden border bg-background shadow-lg animate-nrz-enter md:rounded-xl",
                role: "dialog",
                aria_label: "Editor: {name}",
                // Kopf
                div { class: "flex flex-wrap items-center gap-x-3 gap-y-2 border-b px-4 py-2.5",
                    icons::FileCode { class: "size-4 shrink-0 text-muted-foreground" }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex items-center gap-2",
                            span { class: "truncate font-medium", "{name}" }
                            if dirty() {
                                span { class: "inline-flex shrink-0 items-center gap-1 rounded-full bg-warning/15 px-2 py-0.5 text-xs font-medium text-warning",
                                    span { class: "size-1.5 rounded-full bg-warning" }
                                    "Ungespeichert"
                                }
                            }
                        }
                        div { class: "truncate font-mono text-xs text-muted-foreground", "{path}" }
                    }
                    label { class: "flex items-center gap-2 text-xs text-muted-foreground",
                        icons::WrapText { class: "size-4" }
                        span { class: "hidden sm:inline", "Zeilenumbruch" }
                        Toggle { checked: wrap(), label: "Zeilenumbruch", onchange: move |v| wrap.set(v) }
                    }
                    Button { size: ButtonSize::Sm, disabled: !dirty() || saving(), onclick: move |_| save.call(()),
                        if saving() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::Save { class: "size-4" }
                        }
                        "Speichern"
                    }
                    button { class: ICON_BUTTON, title: "Schliessen (Esc)", aria_label: "Schliessen", onclick: move |_| request_close.call(()),
                        icons::X { class: "size-4" }
                    }
                }
                textarea {
                    class: "min-h-0 flex-1 resize-none overflow-auto bg-transparent p-4 font-mono text-xs leading-5 outline-none {wrap_cls}",
                    spellcheck: "false",
                    autocomplete: "off",
                    wrap: if wrap() { "soft" } else { "off" },
                    value: "{content}",
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    oninput: move |e| content.set(e.value()),
                }
                // Fusszeile
                div { class: "flex flex-wrap items-center justify-between gap-2 border-t px-4 py-1.5 text-xs text-muted-foreground",
                    span { "{lines} Zeilen · {fmt_bytes(bytes)}" if size > 0 && size != bytes { " (auf dem Server {fmt_bytes(size)})" } }
                    span { class: "hidden sm:inline",
                        kbd { class: "rounded border bg-muted px-1 font-mono", "Strg/⌘ S" }
                        " speichert · "
                        kbd { class: "rounded border bg-muted px-1 font-mono", "Esc" }
                        " schliesst"
                    }
                }
            }
        }
        if confirm_close() {
            ConfirmDialog {
                title: "Ungespeicherte Änderungen",
                message: "Deine Änderungen an „{name}“ gehen verloren, wenn du den Editor jetzt schliesst.",
                confirm_label: "Verwerfen",
                destructive: true,
                oncancel: move |_| confirm_close.set(false),
                onconfirm: move |_| {
                    confirm_close.set(false);
                    onclose.call(ever_saved());
                },
            }
        }
    }
}
