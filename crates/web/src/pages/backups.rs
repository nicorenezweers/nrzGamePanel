//! Backups: Liste, erstellen (Welten / ganzer Ordner), herunterladen, wiederherstellen, löschen.

use dioxus::prelude::*;
use nrz_gp_common::{Backup, CreateBackup, Game, ServerStatus, routes};

use crate::{
    Route, api,
    components::{
        CARD, ConfirmDialog, ErrorBox, Loading, Modal, Notice, Tag, fmt_ago, fmt_bytes, fmt_datetime,
        ui::{
            app_shell::EmptyState,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::use_server,
    store::{toast, toast_err},
};

/// Welt-Backups enden auf `_welt.zip` (siehe Backend).
fn is_world_backup(name: &str) -> bool {
    name.ends_with("_welt.zip")
}

#[derive(Clone, PartialEq)]
enum Pending {
    Restore(String),
    Delete(String),
}

#[component]
pub fn ServerBackups(id: String) -> Element {
    let ctx = use_server();
    let status = *ctx.status.read();
    let game = ctx.game();
    let stopped = matches!(status, ServerStatus::Stopped | ServerStatus::Crashed);

    let mut reload = use_signal(|| 0u32);
    let list = use_resource(move || {
        let id = ctx.id();
        let _ = reload();
        async move { api::get::<Vec<Backup>>(&routes::server(&id, "backups")).await }
    });

    let mut show_create = use_signal(|| false);
    let mut worlds_only = use_signal(|| true);
    let mut creating = use_signal(|| false);
    let mut pending = use_signal(|| None::<Pending>);
    let mut busy = use_signal(|| None::<String>);

    let create = move |_| {
        let id = ctx.id();
        creating.set(true);
        spawn(async move {
            let body = CreateBackup { worlds_only: worlds_only() };
            match api::post::<_, Backup>(&routes::server(&id, "backups"), &body).await {
                Ok(b) => {
                    toast(format!("Backup erstellt ({})", fmt_bytes(b.size)));
                    show_create.set(false);
                    reload += 1;
                }
                Err(e) => toast_err(e),
            }
            creating.set(false);
        });
    };

    let confirm = move |_| {
        let Some(p) = pending() else { return };
        pending.set(None);
        let id = ctx.id();
        spawn(async move {
            match p {
                Pending::Restore(name) => {
                    busy.set(Some(name.clone()));
                    let url = routes::server(&id, &format!("backups/{}/restore", routes::enc(&name)));
                    match api::post_empty::<nrz_gp_common::Ok>(&url).await {
                        Ok(_) => toast("Backup wiederhergestellt"),
                        Err(e) => toast_err(e),
                    }
                }
                Pending::Delete(name) => {
                    busy.set(Some(name.clone()));
                    let url = routes::server(&id, &format!("backups/{}", routes::enc(&name)));
                    match api::delete::<nrz_gp_common::Ok>(&url).await {
                        Ok(_) => {
                            toast("Backup gelöscht");
                            reload += 1;
                        }
                        Err(e) => toast_err(e),
                    }
                }
            }
            busy.set(None);
        });
    };

    let world_label = if game == Game::Factorio { "Nur Spielstände" } else { "Nur Welten" };
    let world_hint = if game == Game::Factorio {
        "Sichert den Ordner saves – klein und schnell."
    } else {
        "Sichert die Welt samt Nether und End – klein und schnell."
    };

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-6 p-4 md:p-8",
            div { class: "flex flex-wrap items-end justify-between gap-3",
                div {
                    h2 { class: "text-lg font-semibold tracking-tight", "Backups" }
                    p { class: "text-sm text-muted-foreground", "Sicherungen liegen im Datenordner des Panels und lassen sich jederzeit herunterladen." }
                }
                Button { onclick: move |_| show_create.set(true),
                    icons::Plus { class: "size-4" }
                    "Backup erstellen"
                }
            }

            Notice {
                action: rsx! {
                    Link { to: Route::ServerAutomations { id: id.clone() }, class: "text-sm underline", "Automation anlegen" }
                },
                "Regelmässige Backups richtest du über Automationen ein, z. B. „Backup alle 6 Std“."
            }

            match &*list.read() {
                None => rsx! { Loading { text: "Lade Backups …" } },
                Some(Err(e)) => rsx! { ErrorBox { message: e.clone() } },
                Some(Ok(items)) if items.is_empty() => rsx! {
                    EmptyState {
                        title: "Noch keine Backups",
                        description: "Erstelle jetzt eine Sicherung – vor Updates oder grösseren Änderungen ist das immer eine gute Idee.",
                        icon: rsx! { icons::Archive {} },
                        Button { variant: ButtonVariant::Outline, onclick: move |_| show_create.set(true), "Backup erstellen" }
                    }
                },
                Some(Ok(items)) => {
                    let mut items = items.clone();
                    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                    let total: u64 = items.iter().map(|b| b.size).sum();
                    rsx! {
                        div { class: "{CARD} divide-y",
                            for b in items {
                                {
                                    let name = b.name.clone();
                                    let n_dl = name.clone();
                                    let n_res = name.clone();
                                    let n_del = name.clone();
                                    let id_dl = id.clone();
                                    let is_busy = busy().as_deref() == Some(name.as_str());
                                    rsx! {
                                        div { key: "{name}", class: "flex flex-wrap items-center gap-3 px-4 py-3",
                                            div { class: "flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground",
                                                if is_world_backup(&name) {
                                                    icons::Earth { class: "size-4" }
                                                } else {
                                                    icons::Archive { class: "size-4" }
                                                }
                                            }
                                            div { class: "min-w-0 flex-1",
                                                div { class: "flex flex-wrap items-center gap-2",
                                                    span { class: "truncate font-mono text-sm", "{name}" }
                                                    if is_world_backup(&name) {
                                                        Tag { tone: "info", if game == Game::Factorio { "Spielstände" } else { "Welt" } }
                                                    } else {
                                                        Tag { "Komplett" }
                                                    }
                                                }
                                                div { class: "mt-0.5 text-xs text-muted-foreground",
                                                    "{fmt_datetime(b.created_at)} · {fmt_ago(b.created_at)} · {fmt_bytes(b.size)}"
                                                }
                                            }
                                            div { class: "flex items-center gap-1",
                                                if is_busy {
                                                    icons::LoaderCircle { class: "size-4 animate-spin text-muted-foreground" }
                                                }
                                                Button {
                                                    size: ButtonSize::IconSm,
                                                    variant: ButtonVariant::Ghost,
                                                    title: "Herunterladen",
                                                    aria_label: "Herunterladen",
                                                    onclick: move |_| api::download(&routes::server(&id_dl, &format!("backups/{}/download", routes::enc(&n_dl)))),
                                                    icons::Download { class: "size-4" }
                                                }
                                                Button {
                                                    size: ButtonSize::Sm,
                                                    variant: ButtonVariant::Outline,
                                                    disabled: !stopped || is_busy,
                                                    title: if stopped { "Wiederherstellen" } else { "Nur bei gestopptem Server möglich" },
                                                    onclick: move |_| pending.set(Some(Pending::Restore(n_res.clone()))),
                                                    icons::ArchiveRestore { class: "size-4" }
                                                    span { class: "hidden sm:inline", "Wiederherstellen" }
                                                }
                                                Button {
                                                    size: ButtonSize::IconSm,
                                                    variant: ButtonVariant::Ghost,
                                                    disabled: is_busy,
                                                    title: "Löschen",
                                                    aria_label: "Löschen",
                                                    onclick: move |_| pending.set(Some(Pending::Delete(n_del.clone()))),
                                                    icons::Trash2 { class: "size-4 text-destructive" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        p { class: "px-1 text-xs text-muted-foreground",
                            "Gesamt {fmt_bytes(total)}"
                            if !stopped { " · Wiederherstellen geht nur, wenn der Server gestoppt ist." }
                        }
                    }
                }
            }
        }

        if show_create() {
            Modal {
                title: "Backup erstellen",
                description: "Was soll gesichert werden?",
                onclose: move |_| if !creating() { show_create.set(false) },
                footer: rsx! {
                    Button { variant: ButtonVariant::Outline, onclick: move |_| show_create.set(false), disabled: creating(), "Abbrechen" }
                    Button { onclick: create, disabled: creating(),
                        if creating() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                            "Sichere …"
                        } else {
                            "Backup erstellen"
                        }
                    }
                },
                div { class: "grid gap-2",
                    BackupChoice {
                        selected: worlds_only(),
                        title: world_label,
                        hint: world_hint,
                        onselect: move |_| worlds_only.set(true),
                        icons::Earth { class: "size-5" }
                    }
                    BackupChoice {
                        selected: !worlds_only(),
                        title: "Ganzer Ordner",
                        hint: "Sichert alles: Welten, Einstellungen, Plugins/Mods und Startdateien.",
                        onselect: move |_| worlds_only.set(false),
                        icons::Archive { class: "size-5" }
                    }
                }
                if status == ServerStatus::Running && game == Game::Minecraft {
                    p { class: "mt-3 text-xs text-muted-foreground", "Der Server läuft – das Panel speichert die Welt vorher, damit die Sicherung vollständig ist." }
                }
            }
        }

        match pending() {
            Some(Pending::Restore(name)) => rsx! {
                ConfirmDialog {
                    title: "Backup wiederherstellen?",
                    message: format!(
                        "„{name}“ überschreibt {} des Servers. Änderungen seit dem Backup gehen verloren.",
                        if is_world_backup(&name) { "die aktuelle Welt" } else { "den aktuellen Ordner" }
                    ),
                    confirm_label: "Wiederherstellen",
                    destructive: true,
                    onconfirm: confirm,
                    oncancel: move |_| pending.set(None),
                }
            },
            Some(Pending::Delete(name)) => rsx! {
                ConfirmDialog {
                    title: "Backup löschen?",
                    message: format!("„{name}“ wird endgültig gelöscht."),
                    confirm_label: "Löschen",
                    destructive: true,
                    onconfirm: confirm,
                    oncancel: move |_| pending.set(None),
                }
            },
            None => rsx! {},
        }
    }
}

/// Auswahlkarte im Dialog.
#[component]
fn BackupChoice(selected: bool, #[props(into)] title: String, #[props(into)] hint: String, onselect: EventHandler<()>, children: Element) -> Element {
    let ring = if selected { "border-primary bg-accent/50" } else { "hover:bg-accent/30" };
    rsx! {
        button {
            r#type: "button",
            class: "flex w-full items-start gap-3 rounded-lg border p-3 text-left transition-colors {ring}",
            onclick: move |_| onselect.call(()),
            div { class: "mt-0.5 text-muted-foreground", {children} }
            div { class: "min-w-0 flex-1",
                p { class: "text-sm font-medium", "{title}" }
                p { class: "text-xs text-muted-foreground", "{hint}" }
            }
            if selected {
                icons::CircleCheck { class: "size-4 shrink-0 text-primary" }
            }
        }
    }
}
