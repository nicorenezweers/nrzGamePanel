//! Panel-Einstellungen: Konto, System, Java-Installationen, Import-Ordner.

use dioxus::prelude::*;
use nrz_gp_common::{ChangePassword, SystemInfo};

use crate::{
    api,
    components::{
        CodeBlock, CopyField, ErrorBox, Field, INPUT, Loading, Tag, fmt_bytes,
        ui::{
            app_shell::{PageContainer, PageHeader, PageWidth, SettingsGroup, SettingsRow},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    store::{AUTH, SYSTEM, toast},
};

#[component]
pub fn PanelSettings() -> Element {
    let mut refreshing = use_signal(|| false);
    let refresh = move |_| {
        refreshing.set(true);
        spawn(async move {
            match api::get::<SystemInfo>("/api/system").await {
                Ok(s) => *SYSTEM.write() = Some(s),
                Err(e) => crate::store::toast_err(e),
            }
            refreshing.set(false);
        });
    };
    // Beim Öffnen frische Werte holen (RAM, Speicher)
    use_future(move || async move {
        if let Ok(s) = api::get::<SystemInfo>("/api/system").await {
            *SYSTEM.write() = Some(s);
        }
    });
    let sys = SYSTEM.read().clone();

    rsx! {
        PageContainer { width: PageWidth::Narrow,
            PageHeader { title: "Einstellungen", description: "Konto, System und Umgebung des Panels." }
            div { class: "flex flex-col gap-8",
                AccountSection {}
                match sys {
                    None => rsx! { Loading { text: "Lade Systemdaten …" } },
                    Some(s) => rsx! {
                        SystemSection { sys: s.clone(), refreshing: refreshing(), onrefresh: refresh }
                        JavaSection { sys: s.clone() }
                        ImportRootsSection { sys: s }
                    },
                }
            }
        }
    }
}

#[component]
fn AccountSection() -> Element {
    let user = AUTH.read().as_ref().and_then(|a| a.user.clone()).unwrap_or_default();
    let mut current = use_signal(String::new);
    let mut new = use_signal(String::new);
    let mut repeat = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        if new().chars().count() < 8 {
            error.set(Some("Das neue Passwort muss mindestens 8 Zeichen haben.".into()));
            return;
        }
        if new() != repeat() {
            error.set(Some("Die neuen Passwörter stimmen nicht überein.".into()));
            return;
        }
        error.set(None);
        busy.set(true);
        spawn(async move {
            match api::post::<_, nrz_gp_common::Ok>("/api/auth/password", &ChangePassword { current: current(), new: new() }).await {
                Ok(_) => {
                    toast("Passwort geändert");
                    current.set(String::new());
                    new.set(String::new());
                    repeat.set(String::new());
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    let ready = !current().is_empty() && !new().is_empty() && !repeat().is_empty();

    rsx! {
        SettingsGroup { title: "Konto",
            SettingsRow { label: "Benutzername",
                span { class: "text-sm text-muted-foreground", "{user}" }
            }
            form { class: "flex flex-col gap-4 p-3", onsubmit: submit,
                p { class: "text-sm font-medium", "Passwort ändern" }
                if let Some(e) = error() {
                    ErrorBox { message: e }
                }
                Field { label: "Aktuelles Passwort",
                    input {
                        class: INPUT,
                        r#type: "password",
                        autocomplete: "current-password",
                        value: "{current}",
                        oninput: move |e| current.set(e.value()),
                    }
                }
                div { class: "grid gap-4 sm:grid-cols-2",
                    Field { label: "Neues Passwort", hint: "Mindestens 8 Zeichen.",
                        input {
                            class: INPUT,
                            r#type: "password",
                            autocomplete: "new-password",
                            value: "{new}",
                            oninput: move |e| new.set(e.value()),
                        }
                    }
                    Field { label: "Neues Passwort wiederholen",
                        input {
                            class: INPUT,
                            r#type: "password",
                            autocomplete: "new-password",
                            value: "{repeat}",
                            oninput: move |e| repeat.set(e.value()),
                        }
                    }
                }
                div { class: "flex justify-end",
                    Button { button_type: "submit", disabled: busy() || !ready,
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        }
                        "Passwort ändern"
                    }
                }
            }
        }
    }
}

#[component]
fn SystemSection(sys: SystemInfo, refreshing: bool, onrefresh: EventHandler<()>) -> Element {
    let mem_pct = if sys.memory_total > 0 { sys.memory_used as f64 / sys.memory_total as f64 * 100.0 } else { 0.0 };
    let disk_used = sys.disk_total.saturating_sub(sys.disk_free);
    let disk_pct = if sys.disk_total > 0 { disk_used as f64 / sys.disk_total as f64 * 100.0 } else { 0.0 };
    rsx! {
        SettingsGroup {
            title: "System",
            actions: rsx! {
                Button {
                    size: ButtonSize::Sm,
                    variant: ButtonVariant::Ghost,
                    disabled: refreshing,
                    onclick: move |_| onrefresh.call(()),
                    icons::RefreshCw { class: if refreshing { "size-3.5 animate-spin" } else { "size-3.5" } }
                    "Aktualisieren"
                }
            },
            SettingsRow { label: "Panel-Version",
                span { class: "font-mono text-sm text-muted-foreground", "{sys.panel_version}" }
            }
            SettingsRow { label: "Host",
                span { class: "text-sm text-muted-foreground", "{sys.hostname}" }
            }
            SettingsRow { label: "Betriebssystem",
                span { class: "text-sm text-muted-foreground", "{sys.os}" }
            }
            SettingsRow { label: "CPU",
                span { class: "text-sm text-muted-foreground tabular-nums", "{sys.cpu_count} Kerne · {sys.cpu_usage:.0} % Auslastung" }
            }
            SettingsRow { label: "Arbeitsspeicher",
                Meter { percent: mem_pct, text: format!("{} von {}", fmt_bytes(sys.memory_used), fmt_bytes(sys.memory_total)) }
            }
            SettingsRow { label: "Speicherplatz", hint: "Laufwerk des Datenordners",
                Meter { percent: disk_pct, text: format!("{} frei von {}", fmt_bytes(sys.disk_free), fmt_bytes(sys.disk_total)) }
            }
            SettingsRow { label: "Datenordner", hint: "Server, Backups und Einstellungen",
                CopyField { value: sys.data_dir.clone(), class: "w-full max-w-xs" }
            }
            SettingsRow { label: "Öffentliche Adresse", hint: "Damit verbinden sich Spieler (NRZ_GP_PUBLIC_HOST)",
                CopyField { value: sys.public_host.clone(), class: "w-full max-w-xs" }
            }
        }
    }
}

/// Kleiner Balken mit Text.
#[component]
fn Meter(percent: f64, #[props(into)] text: String) -> Element {
    let bar = if percent > 90.0 {
        "bg-destructive"
    } else if percent > 75.0 {
        "bg-warning"
    } else {
        "bg-primary"
    };
    rsx! {
        div { class: "flex flex-col items-end gap-1",
            span { class: "text-sm text-muted-foreground tabular-nums", "{text}" }
            div { class: "h-1.5 w-40 overflow-hidden rounded-full bg-muted",
                div { class: "h-full rounded-full {bar}", style: "width: {percent.clamp(0.0, 100.0)}%" }
            }
        }
    }
}

#[component]
fn JavaSection(sys: SystemInfo) -> Element {
    let mut java = sys.java.clone();
    java.sort_by(|a, b| b.major.cmp(&a.major));
    rsx! {
        SettingsGroup {
            title: "Java-Installationen",
            footer: "Fehlende Java-Versionen (z. B. 8, 17, 21) lädt das Panel beim Start automatisch herunter (Eclipse Temurin). Pro Server wählt es passend zur Minecraft-Version.",
            if java.is_empty() {
                div { class: "px-3 py-6 text-center text-sm text-muted-foreground", "Noch kein Java gefunden." }
            }
            for j in java {
                div { key: "{j.path}", class: "flex flex-wrap items-center gap-3 px-3 py-2.5",
                    span { class: "flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-sm font-semibold tabular-nums", "{j.major}" }
                    div { class: "min-w-0 flex-1",
                        div { class: "flex flex-wrap items-center gap-2",
                            span { class: "text-sm font-medium", "Java {j.version}" }
                            if let Some(v) = &j.vendor {
                                Tag { "{v}" }
                            }
                        }
                        code { class: "block truncate font-mono text-xs text-muted-foreground", title: "{j.path}", "{j.path}" }
                    }
                }
            }
        }
    }
}

#[component]
fn ImportRootsSection(sys: SystemInfo) -> Element {
    let example = "services:\n  nrzgamepanel:\n    environment:\n      NRZ_GP_IMPORT_ROOTS: /import,/srv/minecraft\n    volumes:\n      - /pfad/auf/dem/host/minecraft:/srv/minecraft";
    rsx! {
        SettingsGroup {
            title: "Import-Ordner",
            footer: "In diesen Ordnern sucht „Server importieren“ nach bestehenden Servern.",
            if sys.import_roots.is_empty() {
                div { class: "px-3 py-6 text-center text-sm text-muted-foreground", "Keine Import-Ordner eingerichtet." }
            }
            for r in sys.import_roots.iter() {
                div { key: "{r}", class: "flex items-center gap-2.5 px-3 py-2.5",
                    icons::FolderOpen { class: "size-4 shrink-0 text-muted-foreground" }
                    code { class: "truncate font-mono text-sm", "{r}" }
                }
            }
        }
        div { class: "-mt-4 flex flex-col gap-3 px-1 text-sm text-muted-foreground",
            p {
                "Weitere Ordner legst du mit der Umgebungsvariable "
                code { class: "font-mono text-foreground", "NRZ_GP_IMPORT_ROOTS" }
                " fest (Komma-getrennt). Läuft das Panel in Docker, muss der Ordner zusätzlich als Volume eingebunden sein, damit das Panel ihn sieht:"
            }
            CodeBlock { label: "docker-compose.yml", code: example }
        }
    }
}
