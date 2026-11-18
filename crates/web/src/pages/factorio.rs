//! Factorio: Spielstände, Mods, Server-Einstellungen (server-settings.json), Admins.

use dioxus::prelude::*;
use nrz_gp_common::{AddonToggle, FactorioMods, FactorioSaves, FactorioSettings, NameList, SaveName, ServerStatus, routes};
use serde_json::{Map, Value};

use crate::{
    Route, api,
    components::{
        CARD, ErrorBox, INPUT, Loading, Modal, Notice, Segmented, TEXTAREA, Tag, Toggle, fmt_bytes, fmt_datetime,
        ui::{
            app_shell::{EmptyState, SettingsGroup, SettingsRow},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::use_server,
    store::{toast, toast_err},
};

#[component]
pub fn FactorioPage(id: String) -> Element {
    let mut tab = use_signal(|| "saves".to_string());
    let items = vec![
        ("saves".to_string(), "Spielstände".to_string()),
        ("mods".to_string(), "Mods".to_string()),
        ("settings".to_string(), "Server-Einstellungen".to_string()),
        ("admins".to_string(), "Admins".to_string()),
    ];
    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-6 p-4 md:p-8",
            div { class: "overflow-x-auto",
                Segmented { items, active: tab(), onchange: move |v| tab.set(v) }
            }
            match tab().as_str() {
                "mods" => rsx! { ModsTab { key: "mods-{id}", id: id.clone() } },
                "settings" => rsx! { SettingsTab { key: "settings-{id}", id: id.clone() } },
                "admins" => rsx! { AdminsTab { key: "admins-{id}", id: id.clone() } },
                _ => rsx! { SavesTab { key: "saves-{id}", id: id.clone() } },
            }
        }
    }
}

/// Kopf eines Reiters: Titel, Beschreibung, Knöpfe rechts.
#[component]
fn TabHeader(#[props(into)] title: String, #[props(into)] description: String, #[props(optional)] actions: Option<Element>) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-end justify-between gap-3",
            div { class: "min-w-0",
                h2 { class: "font-semibold", "{title}" }
                p { class: "text-sm text-muted-foreground", "{description}" }
            }
            if let Some(a) = actions {
                div { class: "flex flex-wrap items-center gap-2", {a} }
            }
        }
    }
}

/// Zeitstempel in Sekunden oder Millisekunden → Datum.
fn fmt_modified(t: i64) -> String {
    fmt_datetime(if t > 100_000_000_000 { t / 1000 } else { t })
}

/* ------------------------------------------------------------------ */
/*                             Spielstände                            */
/* ------------------------------------------------------------------ */

#[component]
fn SavesTab(id: String) -> Element {
    let ctx = use_server();
    let rid = id.clone();
    let mut res = use_resource(move || {
        let id = rid.clone();
        async move { api::get::<FactorioSaves>(&routes::server(&id, "factorio/saves")).await }
    });
    let mut new_open = use_signal(|| false);
    let mut new_name = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let status = *ctx.status.read();
    let stopped = matches!(status, ServerStatus::Stopped | ServerStatus::Crashed);

    let sel_id = id.clone();
    let select = use_callback(move |name: Option<String>| {
        let id = sel_id.clone();
        spawn(async move {
            let latest = name.is_none();
            match api::post::<_, nrz_gp_common::Ok>(&routes::server(&id, "factorio/saves/select"), &SaveName { name }).await {
                Ok(_) => {
                    toast(if latest { "Beim Start wird der neueste Spielstand geladen" } else { "Spielstand für den nächsten Start gewählt" });
                    res.restart();
                }
                Err(e) => toast_err(e),
            }
        });
    });

    let create_id = id.clone();
    let create = use_callback(move |_: ()| {
        let name = new_name().trim().to_string();
        if name.is_empty() || busy() {
            return;
        }
        let id = create_id.clone();
        busy.set(true);
        spawn(async move {
            match api::post::<_, nrz_gp_common::Ok>(&routes::server(&id, "factorio/saves"), &SaveName { name: Some(name) }).await {
                Ok(_) => {
                    toast("Neue Karte erzeugt");
                    new_open.set(false);
                    res.restart();
                }
                Err(e) => toast_err(e),
            }
            busy.set(false);
        });
    });

    let data = res.read().clone();

    rsx! {
        TabHeader {
            title: "Spielstände",
            description: "Welche Karte beim Start geladen wird, neue Karten und Downloads.",
            actions: rsx! {
                Button {
                    size: ButtonSize::Sm,
                    disabled: !stopped,
                    title: if stopped { "" } else { "Nur möglich, wenn der Server gestoppt ist" },
                    onclick: move |_| {
                        new_name.set(format!("karte-{}", js_sys::Date::new_0().get_full_year()));
                        new_open.set(true);
                    },
                    icons::Plus { class: "size-4" }
                    "Neue Karte"
                }
            },
        }
        if !stopped {
            p { class: "-mt-3 text-xs text-muted-foreground", "Neue Karten können nur bei gestopptem Server erzeugt werden." }
        }
        match data {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e } },
            Some(Ok(s)) if s.saves.is_empty() => rsx! {
                EmptyState {
                    title: "Noch keine Spielstände",
                    description: "Erzeuge eine neue Karte oder lade eine .zip über „Dateien“ in den Ordner saves/ hoch.",
                    icon: rsx! { icons::Map {} },
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        href: Route::ServerFiles { id: id.clone() }.to_string(),
                        icons::Folder { class: "size-4" }
                        "Zu den Dateien"
                    }
                }
            },
            Some(Ok(s)) => {
                let selected = s.selected.clone();
                let latest_action = selected.is_some().then(|| rsx! {
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| select.call(None),
                        "Immer neuesten laden"
                    }
                });
                rsx! {
                    Notice {
                        action: latest_action,
                        match &selected {
                            Some(n) => rsx! { "Beim Start wird „{n}“ geladen." },
                            None => rsx! { "Beim Start wird automatisch der neueste Spielstand geladen." },
                        }
                    }
                    div { class: "divide-y {CARD}",
                        for save in s.saves.iter().cloned() {
                            {
                                let is_sel = selected.as_deref() == Some(save.name.as_str());
                                let dl = routes::server(&id, &format!("files/download?path={}", routes::enc(&format!("saves/{}.zip", save.name))));
                                let name = save.name.clone();
                                rsx! {
                                    div { key: "{save.name}", class: "flex flex-wrap items-center gap-3 px-4 py-3",
                                        icons::Save { class: "size-4 shrink-0 text-muted-foreground" }
                                        div { class: "min-w-0 flex-1",
                                            div { class: "flex flex-wrap items-center gap-2",
                                                span { class: "truncate text-sm font-medium", "{save.name}" }
                                                if save.active {
                                                    Tag { tone: "success", "aktiv" }
                                                }
                                                if is_sel {
                                                    Tag { tone: "info", "beim Start" }
                                                }
                                            }
                                            p { class: "text-xs text-muted-foreground", "{fmt_bytes(save.size)} · {fmt_modified(save.modified)}" }
                                        }
                                        div { class: "flex items-center gap-1",
                                            if !is_sel {
                                                Button {
                                                    size: ButtonSize::Sm,
                                                    variant: ButtonVariant::Outline,
                                                    onclick: move |_| select.call(Some(name.clone())),
                                                    "Beim Start laden"
                                                }
                                            }
                                            Button {
                                                size: ButtonSize::IconSm,
                                                variant: ButtonVariant::Ghost,
                                                title: "Herunterladen",
                                                aria_label: "Herunterladen",
                                                onclick: move |_| api::download(&dl),
                                                icons::Download { class: "size-4" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if new_open() {
            Modal {
                title: "Neue Karte erzeugen",
                description: "Erzeugt einen neuen Spielstand mit Standard-Kartenoptionen.",
                onclose: move |_| new_open.set(false),
                footer: rsx! {
                    Button { variant: ButtonVariant::Outline, onclick: move |_| new_open.set(false), "Abbrechen" }
                    Button {
                        disabled: busy() || new_name().trim().is_empty(),
                        onclick: move |_| create.call(()),
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        }
                        "Erzeugen"
                    }
                },
                label { class: "flex flex-col gap-1.5",
                    span { class: "text-sm font-medium", "Name" }
                    input {
                        class: INPUT,
                        autofocus: true,
                        value: "{new_name}",
                        oninput: move |e| new_name.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                create.call(());
                            }
                        },
                    }
                    span { class: "text-xs text-muted-foreground", "Wird als saves/<Name>.zip gespeichert. Das Erzeugen kann einen Moment dauern." }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                                Mods                                */
/* ------------------------------------------------------------------ */

#[component]
fn ModsTab(id: String) -> Element {
    let rid = id.clone();
    let mut res = use_resource(move || {
        let id = rid.clone();
        async move { api::get::<FactorioMods>(&routes::server(&id, "factorio/mods")).await }
    });
    let tid = id.clone();
    let toggle = use_callback(move |(name, enabled): (String, bool)| {
        let id = tid.clone();
        spawn(async move {
            match api::post::<_, nrz_gp_common::Ok>(&routes::server(&id, "factorio/mods/toggle"), &AddonToggle { file: name, enabled }).await {
                Ok(_) => toast(if enabled { "Mod aktiviert – wirksam nach Neustart" } else { "Mod deaktiviert – wirksam nach Neustart" }),
                Err(e) => toast_err(e),
            }
            res.restart();
        });
    });
    let data = res.read().clone();

    rsx! {
        TabHeader { title: "Mods", description: "Aktivieren oder deaktivieren. Änderungen wirken nach einem Neustart." }
        Notice {
            action: rsx! {
                Button {
                    size: ButtonSize::Sm,
                    variant: ButtonVariant::Outline,
                    href: Route::ServerFiles { id: id.clone() }.to_string(),
                    icons::Folder { class: "size-4" }
                    "Dateien öffnen"
                }
            },
            "Neue Mods lädst du als .zip über „Dateien“ in den Ordner mods/ hoch."
        }
        match data {
            None => rsx! { Loading {} },
            Some(Err(e)) => rsx! { ErrorBox { message: e } },
            Some(Ok(m)) if m.mods.is_empty() => rsx! {
                EmptyState { title: "Keine Mods", description: "Im Ordner mods/ liegen noch keine Mods.", icon: rsx! { icons::Puzzle {} } }
            },
            Some(Ok(m)) => rsx! {
                div { class: "divide-y {CARD}",
                    for md in m.mods.iter().cloned() {
                        {
                            let builtin = md.file.is_none();
                            let locked = md.name == "base";
                            let name = md.name.clone();
                            rsx! {
                                div { key: "{md.name}", class: "flex items-center gap-3 px-4 py-3",
                                    icons::Puzzle { class: "size-4 shrink-0 text-muted-foreground" }
                                    div { class: "min-w-0 flex-1",
                                        div { class: "flex flex-wrap items-center gap-2",
                                            span { class: "truncate text-sm font-medium", "{md.name}" }
                                            if let Some(v) = &md.version {
                                                span { class: "text-xs text-muted-foreground tabular-nums", "{v}" }
                                            }
                                            if builtin {
                                                Tag { "eingebaut" }
                                            }
                                        }
                                        if let Some(f) = &md.file {
                                            p { class: "truncate font-mono text-xs text-muted-foreground", "{f}" }
                                        }
                                    }
                                    Toggle {
                                        checked: md.enabled,
                                        disabled: locked,
                                        label: format!("{} aktiv", md.name),
                                        onchange: move |v: bool| toggle.call((name.clone(), v)),
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

/* ------------------------------------------------------------------ */
/*                          Server-Einstellungen                      */
/* ------------------------------------------------------------------ */

#[derive(PartialEq)]
enum Kind {
    Text,
    LongText,
    Tags,
    Num,
    Bool,
    Secret,
}

struct Known {
    path: &'static str,
    label: &'static str,
    hint: &'static str,
    kind: Kind,
}

const K_PUBLIC: &[Known] = &[
    Known { path: "name", label: "Name", hint: "In der Serverliste", kind: Kind::Text },
    Known { path: "description", label: "Beschreibung", hint: "", kind: Kind::LongText },
    Known { path: "tags", label: "Stichwörter", hint: "Durch Komma getrennt", kind: Kind::Tags },
    Known { path: "max_players", label: "Max. Spieler", hint: "0 = unbegrenzt", kind: Kind::Num },
    Known { path: "visibility.public", label: "Öffentlich auffindbar", hint: "Braucht Factorio-Benutzername und Token", kind: Kind::Bool },
    Known { path: "visibility.lan", label: "Im LAN sichtbar", hint: "", kind: Kind::Bool },
];

const K_ACCESS: &[Known] = &[
    Known { path: "username", label: "Factorio-Benutzername", hint: "Für öffentliche Server", kind: Kind::Text },
    Known { path: "token", label: "Token", hint: "Von factorio.com/profile", kind: Kind::Secret },
    Known { path: "game_password", label: "Spielpasswort", hint: "Leer = ohne Passwort", kind: Kind::Secret },
    Known { path: "require_user_verification", label: "Konten prüfen", hint: "Nur Spieler mit gültigem Factorio-Konto", kind: Kind::Bool },
];

const K_GAME: &[Known] = &[
    Known { path: "autosave_interval", label: "Autosave-Intervall (Minuten)", hint: "", kind: Kind::Num },
    Known { path: "autosave_slots", label: "Autosave-Plätze", hint: "Anzahl rotierender Autosaves", kind: Kind::Num },
    Known { path: "afk_autokick_interval", label: "AFK-Kick (Minuten)", hint: "0 = nie", kind: Kind::Num },
    Known { path: "auto_pause", label: "Automatisch pausieren", hint: "Wenn niemand online ist", kind: Kind::Bool },
    Known { path: "only_admins_can_pause_the_game", label: "Nur Admins dürfen pausieren", hint: "", kind: Kind::Bool },
    Known { path: "non_blocking_saving", label: "Speichern ohne Unterbrechung", hint: "Experimentell, nur Linux", kind: Kind::Bool },
];

/// Oberste Schlüssel, die das Formular bearbeitet; alles andere landet im JSON-Feld.
const KNOWN_TOP: &[&str] = &[
    "name",
    "description",
    "tags",
    "max_players",
    "visibility",
    "username",
    "token",
    "game_password",
    "require_user_verification",
    "autosave_interval",
    "autosave_slots",
    "afk_autokick_interval",
    "auto_pause",
    "only_admins_can_pause_the_game",
    "non_blocking_saving",
];

fn jget<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |acc, k| acc.get(k))
}

fn jset(v: &mut Value, path: &str, new: Value) {
    if !v.is_object() {
        *v = Value::Object(Map::new());
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = v;
    for p in &parts[..parts.len() - 1] {
        if !cur.get(*p).is_some_and(Value::is_object) {
            cur[*p] = Value::Object(Map::new());
        }
        cur = &mut cur[*p];
    }
    cur[parts[parts.len() - 1]] = new;
}

fn others_json(v: &Value) -> String {
    let map: Map<String, Value> = v
        .as_object()
        .map(|m| m.iter().filter(|(k, _)| !KNOWN_TOP.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

#[component]
fn SettingsTab(id: String) -> Element {
    let rid = id.clone();
    let res = use_resource(move || {
        let id = rid.clone();
        async move { api::get::<FactorioSettings>(&routes::server(&id, "factorio/settings")).await }
    });
    let data = res.read().clone();
    match data {
        None => rsx! { Loading {} },
        Some(Err(e)) => rsx! { ErrorBox { message: e } },
        Some(Ok(s)) => rsx! { SettingsForm { id, initial: s.settings } },
    }
}

#[component]
fn SecretInput(value: String, oninput: EventHandler<String>) -> Element {
    let mut shown = use_signal(|| false);
    rsx! {
        div { class: "relative w-full sm:w-72",
            input {
                class: "{INPUT} pr-9",
                r#type: if shown() { "text" } else { "password" },
                autocomplete: "off",
                value: "{value}",
                oninput: move |e| oninput.call(e.value()),
            }
            button {
                r#type: "button",
                class: "absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-1 text-muted-foreground hover:text-foreground",
                title: if shown() { "Verbergen" } else { "Anzeigen" },
                onclick: move |_| shown.toggle(),
                if shown() {
                    icons::EyeOff { class: "size-3.5" }
                } else {
                    icons::Eye { class: "size-3.5" }
                }
            }
        }
    }
}

fn setting_row(k: &'static Known, value: &Value, mut sig: Signal<Value>) -> Element {
    let path = k.path;
    let hint = if k.hint.is_empty() { None } else { Some(k.hint.to_string()) };
    let cur = jget(value, path).cloned().unwrap_or(Value::Null);
    let text = match &cur {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    };
    let control = match k.kind {
        Kind::Bool => rsx! {
            Toggle {
                checked: cur.as_bool().unwrap_or(false),
                label: k.label,
                onchange: move |v: bool| jset(&mut sig.write(), path, Value::Bool(v)),
            }
        },
        Kind::Num => rsx! {
            input {
                class: "{INPUT} w-28",
                r#type: "number",
                min: "0",
                value: "{text}",
                oninput: move |e| {
                    let v = e.value().trim().parse::<i64>().map(Value::from).unwrap_or(Value::Null);
                    jset(&mut sig.write(), path, v);
                },
            }
        },
        Kind::Text => rsx! {
            input {
                class: "{INPUT} w-full sm:w-72",
                value: "{text}",
                oninput: move |e| jset(&mut sig.write(), path, Value::String(e.value())),
            }
        },
        Kind::LongText => rsx! {
            textarea {
                class: "{TEXTAREA} min-h-16 w-full sm:w-72",
                value: "{text}",
                oninput: move |e| jset(&mut sig.write(), path, Value::String(e.value())),
            }
        },
        Kind::Tags => rsx! {
            input {
                class: "{INPUT} w-full sm:w-72",
                value: "{text}",
                placeholder: "z. B. vanilla, deutsch",
                oninput: move |e| {
                    let tags: Vec<Value> = e.value().split(',').map(|t| t.trim()).filter(|t| !t.is_empty()).map(|t| Value::String(t.into())).collect();
                    jset(&mut sig.write(), path, Value::Array(tags));
                },
            }
        },
        Kind::Secret => rsx! {
            SecretInput { value: text.clone(), oninput: move |v: String| jset(&mut sig.write(), path, Value::String(v)) }
        },
    };
    rsx! {
        SettingsRow { key: "{path}", label: k.label, hint, {control} }
    }
}

#[component]
fn SettingsForm(id: String, initial: Value) -> Element {
    let init = if initial.is_object() { initial.clone() } else { Value::Object(Map::new()) };
    let init2 = init.clone();
    let mut orig = use_signal(|| init.clone());
    let mut value = use_signal(|| init2);
    let mut orig_others = use_signal(|| others_json(&initial));
    let mut others = use_signal(|| others_json(&initial));
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let v = value();
    let dirty = v != orig() || others() != orig_others();

    let save = move |_| {
        let parsed: Value = match serde_json::from_str(&others()) {
            Ok(Value::Object(m)) => Value::Object(m),
            Ok(_) => {
                error.set(Some("„Weitere Einstellungen“ muss ein JSON-Objekt sein ({ … }).".into()));
                return;
            }
            Err(e) => {
                error.set(Some(format!("„Weitere Einstellungen“ ist kein gültiges JSON: {e}")));
                return;
            }
        };
        let mut out = value();
        for k in K_PUBLIC.iter().chain(K_ACCESS).chain(K_GAME) {
            if k.kind == Kind::Num && jget(&out, k.path).is_some_and(Value::is_null) {
                error.set(Some(format!("Bitte eine Zahl bei „{}“ angeben.", k.label)));
                return;
            }
        }
        if let (Some(m), Value::Object(extra)) = (out.as_object_mut(), parsed) {
            m.retain(|k, _| KNOWN_TOP.contains(&k.as_str()));
            for (k, v) in extra {
                if !KNOWN_TOP.contains(&k.as_str()) {
                    m.insert(k, v);
                }
            }
        }
        error.set(None);
        saving.set(true);
        let id = id.clone();
        spawn(async move {
            match api::put::<_, nrz_gp_common::Ok>(&routes::server(&id, "factorio/settings"), &FactorioSettings { settings: out.clone() }).await {
                Ok(_) => {
                    let o = others_json(&out);
                    orig_others.set(o.clone());
                    others.set(o);
                    orig.set(out.clone());
                    value.set(out);
                    toast("Server-Einstellungen gespeichert – wirksam nach Neustart");
                }
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };

    rsx! {
        TabHeader {
            title: "Server-Einstellungen",
            description: "Inhalt der server-settings.json. Änderungen wirken nach einem Neustart.",
            actions: rsx! {
                if dirty {
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| {
                            value.set(orig());
                            others.set(orig_others());
                            error.set(None);
                        },
                        "Verwerfen"
                    }
                }
                Button { size: ButtonSize::Sm, disabled: !dirty || saving(), onclick: save,
                    if saving() {
                        icons::LoaderCircle { class: "size-4 animate-spin" }
                    } else {
                        icons::Save { class: "size-4" }
                    }
                    "Speichern"
                }
            },
        }
        if let Some(e) = error() {
            ErrorBox { message: e }
        }
        div { class: "flex max-w-3xl flex-col gap-6",
            SettingsGroup { title: "Sichtbarkeit",
                for k in K_PUBLIC.iter() {
                    {setting_row(k, &v, value)}
                }
            }
            SettingsGroup { title: "Zugang",
                for k in K_ACCESS.iter() {
                    {setting_row(k, &v, value)}
                }
            }
            SettingsGroup { title: "Spiel",
                for k in K_GAME.iter() {
                    {setting_row(k, &v, value)}
                }
            }
            div { class: "grid gap-2",
                div { class: "px-1",
                    h3 { class: "text-sm font-semibold", "Weitere Einstellungen" }
                    p { class: "text-xs text-muted-foreground", "Alle übrigen Schlüssel als JSON-Objekt." }
                }
                textarea {
                    class: "{TEXTAREA} min-h-48 font-mono text-xs",
                    spellcheck: false,
                    value: "{others}",
                    oninput: move |e| others.set(e.value()),
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                               Admins                               */
/* ------------------------------------------------------------------ */

#[component]
fn AdminsTab(id: String) -> Element {
    let mut names = use_signal(|| None::<Vec<String>>);
    let mut error = use_signal(|| None::<String>);
    let mut input = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let lid = id.clone();
    use_future(move || {
        let id = lid.clone();
        async move {
            match api::get::<NameList>(&routes::server(&id, "factorio/admins")).await {
                Ok(l) => names.set(Some(l.names)),
                Err(e) => error.set(Some(e)),
            }
        }
    });

    let sid = id.clone();
    let store = use_callback(move |list: Vec<String>| {
        let id = sid.clone();
        busy.set(true);
        spawn(async move {
            match api::put::<_, nrz_gp_common::Ok>(&routes::server(&id, "factorio/admins"), &NameList { names: list.clone() }).await {
                Ok(_) => {
                    names.set(Some(list));
                    toast("Admin-Liste gespeichert");
                }
                Err(e) => toast_err(e),
            }
            busy.set(false);
        });
    });

    let add = use_callback(move |_: ()| {
        let n = input().trim().to_string();
        if n.is_empty() {
            return;
        }
        let mut list = names().unwrap_or_default();
        if list.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
            toast_err(format!("„{n}“ ist bereits Admin"));
            return;
        }
        list.push(n);
        input.set(String::new());
        store.call(list);
    });

    let Some(list) = names() else {
        return rsx! {
            if let Some(e) = error() {
                ErrorBox { message: e }
            } else {
                Loading {}
            }
        };
    };

    rsx! {
        TabHeader {
            title: "Admins",
            description: "Admins dürfen Befehle wie /kick, /ban und /promote nutzen. Gilt ab dem nächsten Start; im Spiel geht auch /promote <Name>.",
        }
        div { class: "flex max-w-xl flex-col gap-4",
            div { class: "flex gap-2",
                input {
                    class: INPUT,
                    placeholder: "Factorio-Benutzername",
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            add.call(());
                        }
                    },
                }
                Button { disabled: busy() || input().trim().is_empty(), onclick: move |_| add.call(()),
                    icons::Plus { class: "size-4" }
                    "Hinzufügen"
                }
            }
            if list.is_empty() {
                EmptyState { title: "Keine Admins", description: "Füge oben einen Benutzernamen hinzu.", icon: rsx! { icons::Shield {} } }
            } else {
                div { class: "divide-y {CARD}",
                    for (i, n) in list.iter().cloned().enumerate() {
                        div { key: "{n}", class: "flex items-center gap-3 px-4 py-2.5",
                            icons::ShieldCheck { class: "size-4 shrink-0 text-muted-foreground" }
                            span { class: "min-w-0 flex-1 truncate text-sm", "{n}" }
                            Button {
                                size: ButtonSize::IconSm,
                                variant: ButtonVariant::Ghost,
                                title: "Entfernen",
                                aria_label: "Entfernen",
                                disabled: busy(),
                                onclick: move |_| {
                                    let mut l = names().unwrap_or_default();
                                    if i < l.len() {
                                        l.remove(i);
                                    }
                                    store.call(l);
                                },
                                icons::X { class: "size-4" }
                            }
                        }
                    }
                }
            }
        }
    }
}
