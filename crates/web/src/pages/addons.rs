//! Plugins bzw. Mods: installierte verwalten (an/aus, löschen, hochladen) und auf Modrinth suchen/installieren.

use std::collections::HashSet;

use dioxus::prelude::*;
use nrz_gp_common::{Addon, AddonList, AddonToggle, Flavor, ModrinthHit, ModrinthInstall, PowerAction, routes};

use crate::{
    Route, api,
    components::{
        ConfirmDialog, ErrorBox, INPUT, Loading, Notice, Segmented, Toggle, drag_has_files, dropped_files, fmt_bytes, fmt_number,
        open_file_picker, take_input_files,
        ui::{
            app_shell::EmptyState,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::{power, use_server},
    store::{toast, toast_err},
};

const UPLOAD_INPUT: &str = "addons-upload-input";

#[component]
pub fn ServerAddons(id: String) -> Element {
    let ctx = use_server();
    let mut tab = use_signal(|| "installed".to_string());
    let mut reload = use_signal(|| 0u32);
    let mut restart_needed = use_signal(|| false);

    let flavor = ctx.summary.read().as_ref().map_or(Flavor::Unknown, |s| s.config.flavor);
    let mods = flavor.uses_mods();
    let (what, what_one) = if mods { ("Mods", "Mod") } else { ("Plugins", "Plugin") };

    if flavor == Flavor::Vanilla {
        return rsx! {
            div { class: "mx-auto flex max-w-7xl flex-col gap-4 p-4 md:p-8",
                EmptyState {
                    title: "Vanilla lädt keine Plugins oder Mods",
                    description: "Für Plugins brauchst du z. B. Paper oder Purpur, für Mods Fabric, Forge oder NeoForge. Die Software kannst du in den Einstellungen wechseln.",
                    icon: rsx! { icons::Puzzle {} },
                    Button { href: Route::ServerSettings { id: id.clone() }.to_string(), variant: ButtonVariant::Outline,
                        icons::Settings2 { class: "size-4" }
                        "Zu den Einstellungen"
                    }
                }
            }
        };
    }

    let online = ctx.status.read().is_online();
    let id_r = id.clone();

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-4 p-4 md:p-8",
            div { class: "flex flex-wrap items-center justify-between gap-3",
                Segmented {
                    items: vec![("installed".to_string(), format!("Installierte {what}")), ("modrinth".to_string(), "Modrinth durchsuchen".to_string())],
                    active: tab(),
                    onchange: move |v| tab.set(v),
                }
            }
            if restart_needed() {
                Notice {
                    warning: true,
                    action: rsx! {
                        if online {
                            Button {
                                size: ButtonSize::Sm,
                                onclick: move |_| {
                                    let id = id_r.clone();
                                    restart_needed.set(false);
                                    spawn(async move { power(&id, PowerAction::Restart).await });
                                },
                                icons::RotateCw { class: "size-4" }
                                "Jetzt neu starten"
                            }
                        }
                    },
                    if online { "Neustart nötig: Änderungen an {what} werden erst nach einem Neustart des Servers wirksam." } else { "Änderungen werden beim nächsten Start des Servers wirksam." }
                }
            }
            if tab() == "installed" {
                Installed {
                    server_id: id.clone(),
                    what,
                    what_one,
                    reload: reload(),
                    onchanged: move |_| restart_needed.set(true),
                    onbrowse: move |_| tab.set("modrinth".into()),
                }
            } else {
                Modrinth {
                    server_id: id.clone(),
                    what,
                    oninstalled: move |_| {
                        restart_needed.set(true);
                        reload += 1;
                    },
                }
            }
        }
    }
}

#[component]
fn Installed(
    server_id: String,
    what: &'static str,
    what_one: &'static str,
    reload: u32,
    onchanged: EventHandler<()>,
    onbrowse: EventHandler<()>,
) -> Element {
    let mut local_reload = use_signal(|| 0u32);
    let mut filter = use_signal(String::new);
    let mut confirm_delete = use_signal(|| None::<Addon>);
    let mut busy = use_signal(HashSet::<String>::new);
    let mut uploading = use_signal(|| false);
    let mut drag_over = use_signal(|| false);
    let sid = use_signal(|| server_id.clone());
    let mut outer = use_signal(|| reload);
    if *outer.peek() != reload {
        outer.set(reload);
    }

    let list = use_resource(move || {
        let id = sid();
        let _ = local_reload();
        let _ = outer();
        async move { api::get::<AddonList>(&routes::server(&id, "mc/addons")).await }
    });

    let state = list.read().clone();
    let folder = match &state {
        Some(Ok(l)) if !l.folder.is_empty() => l.folder.clone(),
        _ => {
            if what == "Mods" {
                "mods".into()
            } else {
                "plugins".into()
            }
        }
    };

    let folder_up = folder.clone();
    let mut do_upload = move |files: Vec<web_sys::File>| {
        let (jars, other): (Vec<_>, Vec<_>) = files.into_iter().partition(|f| f.name().to_lowercase().ends_with(".jar"));
        if !other.is_empty() {
            toast_err(format!("Nur .jar-Dateien – übersprungen: {}", other.iter().map(|f| f.name()).collect::<Vec<_>>().join(", ")));
        }
        if jars.is_empty() {
            return;
        }
        let n = jars.len();
        let id = sid();
        let folder = folder_up.clone();
        uploading.set(true);
        spawn(async move {
            let url = format!("{}?path={}", routes::server(&id, "files/upload"), routes::enc(&folder));
            match api::upload(&url, &jars).await {
                Ok(()) => {
                    toast(if n == 1 { "Datei hochgeladen".to_string() } else { format!("{n} Dateien hochgeladen") });
                    onchanged.call(());
                }
                Err(e) => toast_err(format!("Hochladen fehlgeschlagen: {e}")),
            }
            uploading.set(false);
            local_reload += 1;
        });
    };
    let mut do_upload2 = do_upload.clone();
    let mut do_upload3 = do_upload.clone();

    let f = filter().to_lowercase();
    let addons: Vec<Addon> = match &state {
        Some(Ok(l)) => {
            let mut v: Vec<Addon> = l
                .addons
                .iter()
                .filter(|a| {
                    f.is_empty()
                        || a.file.to_lowercase().contains(&f)
                        || a.name.as_deref().unwrap_or("").to_lowercase().contains(&f)
                        || a.authors.iter().any(|x| x.to_lowercase().contains(&f))
                })
                .cloned()
                .collect();
            v.sort_by_key(|a| a.name.clone().unwrap_or_else(|| a.file.clone()).to_lowercase());
            v
        }
        _ => Vec::new(),
    };
    let total = match &state {
        Some(Ok(l)) => l.addons.len(),
        _ => 0,
    };
    let enabled = match &state {
        Some(Ok(l)) => l.addons.iter().filter(|a| a.enabled).count(),
        _ => 0,
    };

    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "flex flex-wrap items-center gap-2",
                div { class: "relative min-w-48 flex-1",
                    icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                    input {
                        class: "{INPUT} pl-8",
                        placeholder: "{what} durchsuchen",
                        value: "{filter}",
                        oninput: move |e| filter.set(e.value()),
                    }
                }
                if total > 0 {
                    span { class: "text-xs text-muted-foreground", "{enabled} von {total} aktiv" }
                }
                Button {
                    size: ButtonSize::Sm,
                    disabled: uploading(),
                    onclick: move |_| open_file_picker(UPLOAD_INPUT),
                    if uploading() {
                        icons::LoaderCircle { class: "size-4 animate-spin" }
                    } else {
                        icons::Upload { class: "size-4" }
                    }
                    ".jar hochladen"
                }
                input {
                    id: UPLOAD_INPUT,
                    class: "hidden",
                    r#type: "file",
                    accept: ".jar",
                    multiple: true,
                    onchange: move |_| do_upload(take_input_files(UPLOAD_INPUT)),
                }
            }

            div {
                class: "relative",
                ondragenter: move |e| {
                    if drag_has_files(&e) {
                        e.prevent_default();
                        drag_over.set(true);
                    }
                },
                ondragover: move |e| {
                    if drag_has_files(&e) {
                        e.prevent_default();
                    }
                },
                ondrop: move |e| {
                    e.prevent_default();
                    drag_over.set(false);
                    do_upload2(dropped_files(&e));
                },
                if drag_over() {
                    div {
                        class: "absolute inset-0 z-20 flex min-h-40 flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed border-foreground/40 bg-background/85 text-sm",
                        ondragover: move |e| e.prevent_default(),
                        ondragleave: move |_| drag_over.set(false),
                        ondrop: move |e| {
                            e.prevent_default();
                            e.stop_propagation();
                            drag_over.set(false);
                            do_upload3(dropped_files(&e));
                        },
                        icons::Upload { class: "pointer-events-none size-6 text-muted-foreground" }
                        span { class: "pointer-events-none font-medium", "Loslassen, um nach {folder}/ hochzuladen" }
                    }
                }
                match &state {
                    None => rsx! { Loading { text: "Lade {what} …" } },
                    Some(Err(e)) => rsx! {
                        div { class: "flex flex-col items-start gap-3",
                            ErrorBox { message: e.clone() }
                            Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, onclick: move |_| local_reload += 1, "Erneut versuchen" }
                        }
                    },
                    Some(Ok(_)) if total == 0 => rsx! {
                        EmptyState {
                            title: "Noch keine {what}",
                            description: "Lade .jar-Dateien hoch (auch per Drag & Drop hierher) oder installiere {what} direkt von Modrinth.",
                            icon: rsx! { icons::Puzzle {} },
                            Button { variant: ButtonVariant::Outline, onclick: move |_| onbrowse.call(()),
                                icons::Search { class: "size-4" }
                                "Modrinth durchsuchen"
                            }
                        }
                    },
                    Some(Ok(_)) if addons.is_empty() => rsx! {
                        div { class: "rounded-xl border border-dashed px-4 py-8 text-center text-sm text-muted-foreground", "Nichts gefunden für „{filter}“." }
                    },
                    Some(Ok(_)) => rsx! {
                        div { class: "rounded-xl border bg-card shadow-sm",
                            for a in addons.iter().cloned() {
                                {
                                    let title = a.name.clone().unwrap_or_else(|| a.file.trim_end_matches(".disabled").trim_end_matches(".jar").to_string());
                                    let is_busy = busy.read().contains(&a.file);
                                    let file = a.file.clone();
                                    let a2 = a.clone();
                                    rsx! {
                                        div { key: "{a.file}", class: "flex items-start gap-3 border-b px-4 py-3 last:border-b-0",
                                            div { class: if a.enabled { "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted" } else { "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted opacity-50" },
                                                icons::Puzzle { class: "size-4 text-muted-foreground" }
                                            }
                                            div { class: if a.enabled { "min-w-0 flex-1" } else { "min-w-0 flex-1 opacity-60" },
                                                div { class: "flex flex-wrap items-center gap-x-2 gap-y-0.5",
                                                    span { class: "font-medium", "{title}" }
                                                    if let Some(v) = a.version.clone() {
                                                        span { class: "rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground", "{v}" }
                                                    }
                                                    if !a.enabled {
                                                        span { class: "rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground", "Deaktiviert" }
                                                    }
                                                }
                                                if !a.authors.is_empty() {
                                                    p { class: "text-xs text-muted-foreground", "von {a.authors.join(\", \")}" }
                                                }
                                                if let Some(d) = a.description.clone().filter(|d| !d.trim().is_empty()) {
                                                    p { class: "mt-1 line-clamp-2 text-sm text-muted-foreground", "{d}" }
                                                }
                                                p { class: "mt-1 truncate font-mono text-[11px] text-muted-foreground", "{a.file} · {fmt_bytes(a.size)}" }
                                            }
                                            div { class: "flex shrink-0 items-center gap-2",
                                                if is_busy {
                                                    icons::LoaderCircle { class: "size-4 animate-spin text-muted-foreground" }
                                                }
                                                Toggle {
                                                    checked: a.enabled,
                                                    disabled: is_busy,
                                                    label: if a.enabled { format!("{title} deaktivieren") } else { format!("{title} aktivieren") },
                                                    onchange: move |v: bool| {
                                                        let id = sid();
                                                        let file = file.clone();
                                                        busy.write().insert(file.clone());
                                                        spawn(async move {
                                                            match api::post::<_, nrz_gp_common::Ok>(&routes::server(&id, "mc/addons/toggle"), &AddonToggle { file: file.clone(), enabled: v }).await {
                                                                Ok(_) => {
                                                                    toast(if v { "Aktiviert" } else { "Deaktiviert" });
                                                                    onchanged.call(());
                                                                }
                                                                Err(e) => toast_err(e),
                                                            }
                                                            busy.write().remove(&file);
                                                            local_reload += 1;
                                                        });
                                                    },
                                                }
                                                button {
                                                    class: "inline-flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive/10 hover:text-destructive",
                                                    title: "Löschen",
                                                    aria_label: "{title} löschen",
                                                    onclick: move |_| confirm_delete.set(Some(a2.clone())),
                                                    icons::Trash2 { class: "size-4" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            p { class: "text-xs text-muted-foreground", "Ordner: {folder}/ · Deaktivierte {what} werden in .disabled umbenannt und nicht geladen." }
        }

        if let Some(a) = confirm_delete() {
            ConfirmDialog {
                title: "{what_one} löschen?",
                message: "„{a.name.clone().unwrap_or_else(|| a.file.clone())}“ ({a.file}) wird vom Server gelöscht. Konfigurationsdateien bleiben erhalten.",
                confirm_label: "Löschen",
                destructive: true,
                oncancel: move |_| confirm_delete.set(None),
                onconfirm: move |_| {
                    confirm_delete.set(None);
                    let id = sid();
                    let file = a.file.clone();
                    spawn(async move {
                        match api::delete::<nrz_gp_common::Ok>(&routes::server(&id, &format!("mc/addons/{}", routes::enc(&file)))).await {
                            Ok(_) => {
                                toast("Gelöscht");
                                onchanged.call(());
                            }
                            Err(e) => toast_err(e),
                        }
                        local_reload += 1;
                    });
                },
            }
        }
    }
}

#[component]
fn Modrinth(server_id: String, what: &'static str, oninstalled: EventHandler<()>) -> Element {
    let sid = use_signal(|| server_id.clone());
    let mut input = use_signal(String::new);
    let mut query = use_signal(String::new);
    let mut generation = use_signal(|| 0u32);
    let mut installing = use_signal(HashSet::<String>::new);
    let mut installed = use_signal(HashSet::<String>::new);

    let results = use_resource(move || {
        let id = sid();
        let q = query();
        async move { api::get::<Vec<ModrinthHit>>(&format!("{}?q={}", routes::server(&id, "mc/modrinth/search"), routes::enc(q.trim()))).await }
    });

    let mut on_input = move |v: String| {
        input.set(v);
        generation += 1;
        let g = *generation.peek();
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(350).await;
            if *generation.peek() == g {
                let v = input.peek().clone();
                if *query.peek() != v {
                    query.set(v);
                }
            }
        });
    };

    let state = results.read().clone();

    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "relative",
                icons::Search { class: "pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" }
                input {
                    class: "{INPUT} h-10 pl-9",
                    placeholder: "{what} auf Modrinth suchen, z. B. LuckPerms, WorldEdit, Lithium …",
                    value: "{input}",
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    oninput: move |e| on_input(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            query.set(input());
                        }
                    },
                }
                if matches!(state, None) {
                    icons::LoaderCircle { class: "absolute top-1/2 right-3 size-4 -translate-y-1/2 animate-spin text-muted-foreground" }
                }
            }
            p { class: "-mt-2 text-xs text-muted-foreground",
                if query().trim().is_empty() { "Beliebte Projekte, passend zu Software und Version dieses Servers." } else { "Treffer passend zu Software und Version dieses Servers." }
            }
            match state {
                None => rsx! { Loading { text: "Suche auf Modrinth …" } },
                Some(Err(e)) => rsx! { ErrorBox { message: "Modrinth nicht erreichbar: {e}" } },
                Some(Ok(hits)) if hits.is_empty() => rsx! {
                    EmptyState {
                        title: "Keine Treffer",
                        description: "Versuche einen anderen Suchbegriff. Es werden nur Projekte angezeigt, die zu dieser Server-Software passen.",
                        icon: rsx! { icons::Search {} },
                    }
                },
                Some(Ok(hits)) => rsx! {
                    div { class: "grid gap-3 md:grid-cols-2",
                        for h in hits {
                            {
                                let is_installing = installing.read().contains(&h.project_id);
                                let done = installed.read().contains(&h.project_id);
                                let pid = h.project_id.clone();
                                let title = h.title.clone();
                                rsx! {
                                    div { key: "{h.project_id}", class: "flex gap-3 rounded-xl border bg-card p-4 shadow-sm",
                                        HitIcon { url: h.icon_url.clone() }
                                        div { class: "flex min-w-0 flex-1 flex-col gap-1",
                                            div { class: "flex items-start justify-between gap-2",
                                                div { class: "min-w-0",
                                                    a {
                                                        class: "font-medium hover:underline",
                                                        href: "https://modrinth.com/project/{h.slug}",
                                                        target: "_blank",
                                                        rel: "noopener noreferrer",
                                                        "{h.title}"
                                                    }
                                                    p { class: "text-xs text-muted-foreground",
                                                        "von {h.author} · "
                                                        span { class: "inline-flex items-center gap-0.5",
                                                            icons::Download { class: "size-3" }
                                                            "{fmt_number(h.downloads)}"
                                                        }
                                                    }
                                                }
                                                Button {
                                                    size: ButtonSize::Sm,
                                                    variant: if done { ButtonVariant::Ghost } else { ButtonVariant::Outline },
                                                    disabled: is_installing || done,
                                                    onclick: move |_| {
                                                        let id = sid();
                                                        let pid = pid.clone();
                                                        let title = title.clone();
                                                        installing.write().insert(pid.clone());
                                                        spawn(async move {
                                                            match api::post::<_, Addon>(&routes::server(&id, "mc/modrinth/install"), &ModrinthInstall { project_id: pid.clone() }).await {
                                                                Ok(a) => {
                                                                    toast(format!("{} installiert", a.name.unwrap_or(title)));
                                                                    installed.write().insert(pid.clone());
                                                                    oninstalled.call(());
                                                                }
                                                                Err(e) => toast_err(format!("Installation fehlgeschlagen: {e}")),
                                                            }
                                                            installing.write().remove(&pid);
                                                        });
                                                    },
                                                    if is_installing {
                                                        icons::LoaderCircle { class: "size-4 animate-spin" }
                                                        "Installiere …"
                                                    } else if done {
                                                        icons::Check { class: "size-4" }
                                                        "Installiert"
                                                    } else {
                                                        icons::CloudDownload { class: "size-4" }
                                                        "Installieren"
                                                    }
                                                }
                                            }
                                            p { class: "line-clamp-2 text-sm text-muted-foreground", "{h.description}" }
                                            if !h.categories.is_empty() {
                                                div { class: "mt-1 flex flex-wrap gap-1",
                                                    for c in h.categories.iter().take(4) {
                                                        span { key: "{c}", class: "rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground", "{c}" }
                                                    }
                                                }
                                            }
                                        }
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

#[component]
fn HitIcon(url: Option<String>) -> Element {
    let mut failed = use_signal(|| false);
    rsx! {
        if let Some(u) = url.filter(|_| !failed()) {
            img {
                class: "size-12 shrink-0 rounded-lg bg-muted object-cover",
                src: "{u}",
                alt: "",
                loading: "lazy",
                onerror: move |_| failed.set(true),
            }
        } else {
            div { class: "flex size-12 shrink-0 items-center justify-center rounded-lg bg-muted",
                icons::Puzzle { class: "size-5 text-muted-foreground" }
            }
        }
    }
}
