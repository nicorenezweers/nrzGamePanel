//! Übersicht: Host-Werte, alle Server als Karten, letzte Ereignisse.

use dioxus::prelude::*;
use nrz_gp_common::{EventKind, Game, GameEvent, PowerAction, ServerStatus, SystemInfo};

use crate::{
    Route, api,
    components::{
        Loading, PlayerHead, StatCard, StatusBadge, fmt_bytes, fmt_time_ms,
        ui::{
            app_shell::{PageContainer, PageHeader, PageWidth},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::power,
    store::{SERVERS, SERVERS_LOADED, SYSTEM},
};

#[component]
pub fn Dashboard() -> Element {
    let mut events = use_signal(Vec::<GameEvent>::new);
    use_future(move || async move {
        loop {
            if let Ok(e) = api::get::<Vec<GameEvent>>("/api/events?limit=30").await {
                events.set(e);
            }
            if let Ok(s) = api::get::<SystemInfo>("/api/system").await {
                *SYSTEM.write() = Some(s);
            }
            gloo_timers::future::TimeoutFuture::new(10_000).await;
        }
    });
    let servers = SERVERS.read().clone();
    let sys = SYSTEM.read().clone();
    let running = servers.iter().filter(|s| s.status == ServerStatus::Running).count();
    let players: usize = servers.iter().map(|s| s.players.len()).sum();
    let nav = navigator();

    rsx! {
        document::Title { "Übersicht · nrzGamePanel" }
        PageContainer { width: PageWidth::Wide,
            PageHeader {
                title: "Übersicht",
                description: "Alle Spielserver auf einen Blick",
                actions: rsx! {
                    Button { variant: ButtonVariant::Outline, href: "/import",
                        icons::FolderInput { class: "size-4" }
                        "Importieren"
                    }
                    Button { href: "/new",
                        icons::Plus { class: "size-4" }
                        "Server erstellen"
                    }
                },
            }
            div { class: "grid grid-cols-2 gap-4 lg:grid-cols-4",
                StatCard { label: "Server", value: format!("{running} / {}", servers.len()), hint: "laufen", icon: rsx! { icons::Server { class: "size-4" } } }
                StatCard { label: "Spieler", value: players.to_string(), hint: "online", icon: rsx! { icons::Users { class: "size-4" } } }
                if let Some(s) = sys.clone() {
                    StatCard {
                        label: "CPU",
                        value: format!("{:.0} %", s.cpu_usage),
                        hint: format!("{} Kerne", s.cpu_count),
                        percent: f64::from(s.cpu_usage),
                        icon: rsx! { icons::Cpu { class: "size-4" } },
                    }
                    StatCard {
                        label: "Arbeitsspeicher",
                        value: fmt_bytes(s.memory_used),
                        hint: format!("von {} · Speicher frei: {}", fmt_bytes(s.memory_total), fmt_bytes(s.disk_free)),
                        percent: if s.memory_total > 0 { s.memory_used as f64 / s.memory_total as f64 * 100.0 } else { 0.0 },
                        icon: rsx! { icons::MemoryStick { class: "size-4" } },
                    }
                }
            }

            div { class: "mt-8 grid gap-6 xl:grid-cols-3",
                div { class: "xl:col-span-2",
                    h2 { class: "mb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Server" }
                    if !SERVERS_LOADED() {
                        Loading {}
                    } else if servers.is_empty() {
                        div { class: "flex flex-col items-center gap-4 rounded-xl border border-dashed px-6 py-14 text-center",
                            div { class: "flex size-12 items-center justify-center rounded-xl bg-muted", icons::Server { class: "size-6" } }
                            div {
                                h3 { class: "font-semibold", "Noch keine Server" }
                                p { class: "mt-1 max-w-md text-sm text-muted-foreground",
                                    "Erstelle einen neuen Minecraft- oder Factorio-Server – oder importiere einen bestehenden. Das Panel erkennt Software, Version und Startdatei automatisch."
                                }
                            }
                            div { class: "flex flex-wrap justify-center gap-2",
                                Button { variant: ButtonVariant::Outline, href: "/import", icons::FolderInput { class: "size-4" } "Bestehenden importieren" }
                                Button { href: "/new", icons::Plus { class: "size-4" } "Server erstellen" }
                            }
                        }
                    } else {
                        div { class: "grid gap-4 md:grid-cols-2",
                            for s in servers {
                                {
                                    let id = s.config.id.clone();
                                    let id_start = id.clone();
                                    let id_stop = id.clone();
                                    let mem_pct = if s.config.memory_mb > 0 { s.stats.memory_bytes as f64 / (f64::from(s.config.memory_mb) * 1024.0 * 1024.0) * 100.0 } else { 0.0 };
                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: "group cursor-pointer rounded-xl border bg-card p-4 shadow-sm transition-colors hover:border-ring/50",
                                            onclick: move |_| { nav.push(Route::ServerOverview { id: id.clone() }); },
                                            div { class: "flex items-start gap-3",
                                                div { class: "flex size-10 shrink-0 items-center justify-center rounded-lg bg-muted",
                                                    if s.config.game == Game::Factorio { icons::Factory { class: "size-5" } } else { icons::Pickaxe { class: "size-5" } }
                                                }
                                                div { class: "min-w-0 flex-1",
                                                    div { class: "truncate font-semibold", "{s.config.name}" }
                                                    div { class: "truncate text-xs text-muted-foreground",
                                                        "{s.config.flavor.label()} {s.config.version.clone().unwrap_or_default()} · Port {s.config.port}"
                                                    }
                                                }
                                                StatusBadge { status: s.status }
                                            }
                                            div { class: "mt-4 grid grid-cols-3 gap-2 text-xs",
                                                div {
                                                    div { class: "text-muted-foreground", "Spieler" }
                                                    div { class: "mt-0.5 font-medium tabular-nums", "{s.players.len()} / {s.stats.players_max}" }
                                                }
                                                div {
                                                    div { class: "text-muted-foreground", "CPU" }
                                                    div { class: "mt-0.5 font-medium tabular-nums", "{s.stats.cpu:.0} %" }
                                                }
                                                div {
                                                    div { class: "text-muted-foreground", "RAM" }
                                                    div { class: "mt-0.5 font-medium tabular-nums",
                                                        "{fmt_bytes(s.stats.memory_bytes)}"
                                                    }
                                                    div { class: "mt-1 h-1 overflow-hidden rounded-full bg-muted",
                                                        div { class: "h-full rounded-full bg-primary", style: "width: {mem_pct.clamp(0.0, 100.0)}%" }
                                                    }
                                                }
                                            }
                                            if !s.players.is_empty() {
                                                div { class: "mt-3 flex -space-x-1",
                                                    for p in s.players.iter().take(8) {
                                                        span { key: "{p}", title: "{p}", PlayerHead { player: p.clone(), size: 5 } }
                                                    }
                                                }
                                            }
                                            if !s.warnings.is_empty() && !s.warnings.iter().all(|w| matches!(w, nrz_gp_common::ServerWarning::RconDisabled)) {
                                                div { class: "mt-3 flex items-center gap-1.5 text-xs text-warning",
                                                    icons::TriangleAlert { class: "size-3.5" }
                                                    "Braucht Aufmerksamkeit"
                                                }
                                            }
                                            div { class: "mt-4 flex gap-2", onclick: move |e| e.stop_propagation(),
                                                if s.status.is_online() {
                                                    Button {
                                                        size: ButtonSize::Sm,
                                                        variant: ButtonVariant::Outline,
                                                        onclick: move |_| {
                                                            let id = id_stop.clone();
                                                            spawn(async move { power(&id, PowerAction::Stop).await });
                                                        },
                                                        icons::Square { class: "size-3.5" }
                                                        "Stoppen"
                                                    }
                                                } else if s.status != ServerStatus::Installing {
                                                    Button {
                                                        size: ButtonSize::Sm,
                                                        onclick: move |_| {
                                                            let id = id_start.clone();
                                                            spawn(async move { power(&id, PowerAction::Start).await });
                                                        },
                                                        icons::Play { class: "size-3.5" }
                                                        "Starten"
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
                div {
                    h2 { class: "mb-3 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Ereignisse" }
                    EventFeed { events: events(), show_server: true }
                }
            }
        }
    }
}

pub fn event_icon(kind: EventKind) -> Element {
    match kind {
        EventKind::PlayerJoin => rsx! { icons::LogIn { class: "size-3.5 text-success" } },
        EventKind::PlayerLeave => rsx! { icons::LogOut { class: "size-3.5 text-muted-foreground" } },
        EventKind::Chat => rsx! { icons::MessageSquare { class: "size-3.5 text-info" } },
        EventKind::PlayerDeath => rsx! { icons::Skull { class: "size-3.5 text-destructive" } },
        EventKind::Advancement => rsx! { icons::Trophy { class: "size-3.5 text-warning" } },
        EventKind::ServerStarted | EventKind::ServerStarting => rsx! { icons::Play { class: "size-3.5 text-success" } },
        EventKind::ServerStopped => rsx! { icons::Square { class: "size-3.5 text-muted-foreground" } },
        EventKind::ServerCrashed | EventKind::LogError => rsx! { icons::CircleAlert { class: "size-3.5 text-destructive" } },
        EventKind::BackupCreated => rsx! { icons::Archive { class: "size-3.5 text-muted-foreground" } },
        EventKind::AutomationRun => rsx! { icons::Zap { class: "size-3.5 text-muted-foreground" } },
    }
}

/// Liste von Ereignissen (neueste zuerst).
#[component]
pub fn EventFeed(events: Vec<GameEvent>, #[props(default)] show_server: bool) -> Element {
    let servers = SERVERS.read().clone();
    rsx! {
        div { class: "divide-y rounded-xl border bg-card shadow-sm",
            if events.is_empty() {
                div { class: "px-4 py-8 text-center text-sm text-muted-foreground", "Noch keine Ereignisse" }
            }
            for e in events.into_iter().filter(|e| e.kind != EventKind::LogError).take(40) {
                div { key: "{e.id}", class: "flex items-start gap-2.5 px-3 py-2",
                    div { class: "mt-0.5", {event_icon(e.kind)} }
                    div { class: "min-w-0 flex-1 text-sm",
                        div { class: "flex items-center gap-1.5",
                            if let Some(p) = &e.player {
                                span { class: "font-medium", "{p}" }
                            }
                            span { class: "truncate text-muted-foreground",
                                if e.kind == EventKind::Chat { "„{e.message}“" } else { "{e.message}" }
                            }
                        }
                        div { class: "text-[11px] text-muted-foreground",
                            "{fmt_time_ms(e.ts)}"
                            if show_server {
                                if let Some(s) = servers.iter().find(|s| s.config.id == e.server_id) {
                                    " · {s.config.name}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
