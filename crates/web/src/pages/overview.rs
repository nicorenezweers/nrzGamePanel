//! Server-Übersicht: Warnungen mit Lösung, Kennzahlen, Spieler, Mini-Konsole, Ereignisse.

use dioxus::prelude::*;
use nrz_gp_common::{ServerStatus, ServerWarning, routes};

use crate::{
    Route, api,
    components::{
        Notice, PlayerHead, StatCard, fmt_bytes, fmt_duration,
        ui::button::{Button, ButtonSize, ButtonVariant},
    },
    layout::use_server,
    pages::{console::ConsoleView, dashboard::EventFeed},
    store::{toast, toast_err},
};

#[component]
pub fn ServerOverview(id: String) -> Element {
    let ctx = use_server();
    let Some(sum) = ctx.summary.read().clone() else { return rsx! {} };
    let stats = ctx.stats.read().clone();
    let players = ctx.players.read().clone();
    let status = *ctx.status.read();
    let mem_limit = u64::from(sum.config.memory_mb) * 1024 * 1024;
    let events = ctx.events.read().clone();

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-6 p-4 md:p-8",
            // Warnungen mit direkter Lösung
            for w in sum.warnings.iter().cloned() {
                {
                    let id = id.clone();
                    match w {
                        ServerWarning::EulaNotAccepted => rsx! {
                            Notice {
                                warning: true,
                                action: rsx! {
                                    Button {
                                        size: ButtonSize::Sm,
                                        onclick: move |_| {
                                            let id = id.clone();
                                            spawn(async move {
                                                match api::post_empty::<nrz_gp_common::Ok>(&routes::server(&id, "eula")).await {
                                                    Ok(_) => { toast("EULA akzeptiert"); ctx.reload(); }
                                                    Err(e) => toast_err(e),
                                                }
                                            });
                                        },
                                        "EULA akzeptieren"
                                    }
                                },
                                "Die "
                                a { class: "underline", href: "https://aka.ms/MinecraftEULA", target: "_blank", "Minecraft-EULA" }
                                " ist noch nicht akzeptiert – ohne startet der Server nicht."
                            }
                        },
                        ServerWarning::RconDisabled => rsx! {
                            Notice {
                                action: rsx! {
                                    Button {
                                        size: ButtonSize::Sm,
                                        variant: ButtonVariant::Outline,
                                        onclick: move |_| {
                                            let id = id.clone();
                                            spawn(async move {
                                                match api::post_empty::<nrz_gp_common::ServerSummary>(&routes::server(&id, "rcon/enable")).await {
                                                    Ok(_) => { toast("RCON eingerichtet – aktiv nach dem nächsten Start"); ctx.reload(); }
                                                    Err(e) => toast_err(e),
                                                }
                                            });
                                        },
                                        "Einrichten"
                                    }
                                },
                                "Live-Daten (Spielerpositionen, Inventare online) brauchen RCON. Das Panel richtet es mit zufälligem Passwort ein."
                            }
                        },
                        ServerWarning::JavaMissing { required } => rsx! {
                            Notice { warning: true,
                                "Java {required} wird benötigt. Das Panel lädt es beim nächsten Start automatisch herunter (Eclipse Temurin)."
                            }
                        },
                        ServerWarning::LaunchFileMissing { path } => rsx! {
                            Notice {
                                warning: true,
                                action: rsx! {
                                    Link { to: Route::ServerSettings { id: id.clone() }, class: "text-sm underline", "Startdatei wählen" }
                                },
                                "Startdatei „{path}“ fehlt."
                            }
                        },
                        ServerWarning::Other { message } => rsx! { Notice { warning: true, "{message}" } },
                    }
                }
            }
            if status == ServerStatus::Installing {
                Notice { "Der Server wird installiert. Den Fortschritt siehst du unten in der Konsole." }
            }

            div { class: "grid grid-cols-2 gap-4 lg:grid-cols-4",
                StatCard {
                    label: "Spieler",
                    value: if status.is_online() { format!("{} / {}", players.len(), stats.players_max) } else { "–".into() },
                    icon: rsx! { icons::Users { class: "size-4" } },
                }
                StatCard {
                    label: "CPU",
                    value: if status.is_online() { format!("{:.0} %", stats.cpu) } else { "–".into() },
                    icon: rsx! { icons::Cpu { class: "size-4" } },
                }
                StatCard {
                    label: "RAM",
                    value: if status.is_online() { fmt_bytes(stats.memory_bytes) } else { "–".into() },
                    hint: if sum.config.game == nrz_gp_common::Game::Minecraft { format!("Limit {}", fmt_bytes(mem_limit)) } else { String::new() },
                    percent: if status.is_online() && mem_limit > 0 { stats.memory_bytes as f64 / mem_limit as f64 * 100.0 } else { 0.0 },
                    icon: rsx! { icons::MemoryStick { class: "size-4" } },
                }
                StatCard {
                    label: "Laufzeit",
                    value: if status.is_online() { fmt_duration(stats.uptime_secs) } else { "–".into() },
                    hint: stats.disk_bytes.map(|d| format!("Ordner: {}", fmt_bytes(d))).unwrap_or_default(),
                    icon: rsx! { icons::Clock { class: "size-4" } },
                }
            }

            div { class: "grid gap-6 xl:grid-cols-3",
                div { class: "flex flex-col gap-3 xl:col-span-2",
                    div { class: "flex items-center justify-between",
                        h2 { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "Konsole" }
                        Link { to: Route::ServerConsole { id: id.clone() }, class: "text-xs text-muted-foreground hover:text-foreground", "Vollbild →" }
                    }
                    ConsoleView { compact: true }
                }
                div { class: "flex flex-col gap-6",
                    div {
                        h2 { class: "mb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Online" }
                        div { class: "rounded-xl border bg-card p-3 shadow-sm",
                            if players.is_empty() {
                                p { class: "py-4 text-center text-sm text-muted-foreground", "Niemand online" }
                            }
                            div { class: "flex flex-col gap-1",
                                for p in players.iter() {
                                    {
                                        let is_mc = sum.config.game == nrz_gp_common::Game::Minecraft;
                                        rsx! {
                                            div { key: "{p}", class: "flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-accent/50",
                                                if is_mc { PlayerHead { player: p.clone(), size: 6 } }
                                                span { class: "flex-1 text-sm", "{p}" }
                                                if is_mc {
                                                    Link {
                                                        to: Route::PlayerPage { id: id.clone(), uuid: p.clone() },
                                                        class: "text-xs text-muted-foreground hover:text-foreground",
                                                        "Inventar"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div {
                        h2 { class: "mb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Ereignisse" }
                        EventFeed { events }
                    }
                }
            }
        }
    }
}
