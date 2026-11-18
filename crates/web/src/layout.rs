//! App-Gerüst: Seitenleiste (Server-Liste), Server-Bereich mit Kopf, Reitern und Live-Verbindung.

use dioxus::prelude::*;
use futures::{SinkExt, StreamExt};
use nrz_gp_common::{
    ConsoleLine, Game, GameEvent, PowerAction, PowerRequest, ServerStats, ServerStatus, ServerSummary, WsClientMessage, WsMessage,
    routes,
};

use crate::{
    Route, api,
    components::{
        StatusBadge, StatusDot, Toaster,
        ui::{
            app_shell::*,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    store::{AUTH, SERVERS, SYSTEM, refresh_servers, toast, toast_err},
};

const LOGO: Asset = asset!("/assets/logo.svg");

#[component]
pub fn Shell() -> Element {
    let route = use_route::<Route>();
    // Serverliste alle 4 s aktualisieren
    use_future(|| async move {
        loop {
            refresh_servers().await;
            gloo_timers::future::TimeoutFuture::new(4000).await;
        }
    });
    use_future(|| async move {
        if let Ok(s) = api::get("/api/system").await {
            *SYSTEM.write() = Some(s);
        }
    });
    let servers = SERVERS.read().clone();
    let user = AUTH.read().as_ref().and_then(|a| a.user.clone()).unwrap_or_default();
    let current_id = route_server_id(&route);
    let version = format!("nrzGamePanel {}", nrz_gp_common::PANEL_VERSION);

    rsx! {
        AppShell {
            header_brand: rsx! {
                img { src: LOGO, class: "size-7 rounded-lg", alt: "" }
                span { class: "font-semibold", "nrzGamePanel" }
            },
            sidebar: rsx! {
                AppBrand {
                    logo: rsx! { img { src: LOGO, alt: "" } },
                    name: "nrzGamePanel",
                    subtitle: "Minecraft & Factorio",
                    href: "/",
                }
                AppNav {
                    AppNavItem {
                        label: "Übersicht",
                        href: "/",
                        active: matches!(route, Route::Dashboard {}),
                        icon: rsx! { icons::LayoutDashboard { class: "size-4" } },
                    }
                    AppNavHeading {
                        label: "Server",
                        action: rsx! {
                            Link {
                                to: Route::CreateServer {},
                                class: "rounded p-0.5 text-muted-foreground hover:bg-sidenav-accent hover:text-foreground",
                                title: "Server hinzufügen",
                                icons::Plus { class: "size-3.5" }
                            }
                        },
                    }
                    for s in servers.iter() {
                        AppNavItem {
                            key: "{s.config.id}",
                            label: s.config.name.clone(),
                            href: format!("/servers/{}", s.config.id),
                            active: current_id.as_deref() == Some(s.config.id.as_str()),
                            icon: rsx! {
                                span { class: "relative flex size-4 items-center justify-center",
                                    if s.config.game == Game::Factorio {
                                        icons::Factory { class: "size-4" }
                                    } else {
                                        icons::Pickaxe { class: "size-4" }
                                    }
                                    span { class: "absolute -right-0.5 -bottom-0.5", StatusDot { status: s.status } }
                                }
                            },
                            count: if s.status.is_online() && !s.players.is_empty() { Some(s.players.len()) } else { None },
                        }
                    }
                    AppNavItem {
                        label: "Server erstellen",
                        href: "/new",
                        active: matches!(route, Route::CreateServer {}),
                        icon: rsx! { icons::CirclePlus { class: "size-4" } },
                    }
                    AppNavItem {
                        label: "Server importieren",
                        href: "/import",
                        active: matches!(route, Route::ImportServer {}),
                        icon: rsx! { icons::FolderInput { class: "size-4" } },
                    }
                    AppNavHeading { label: "Panel" }
                    AppNavItem {
                        label: "Integrationen",
                        href: "/integrations",
                        active: matches!(route, Route::Integrations {}),
                        icon: rsx! { icons::Plug { class: "size-4" } },
                    }
                }
                AppSidebarFooter {
                    AppNavItem {
                        label: "Einstellungen",
                        href: "/settings",
                        active: matches!(route, Route::PanelSettings {}),
                        icon: rsx! { icons::Settings { class: "size-4" } },
                    }
                    AppAccountButton {
                        name: user,
                        detail: "Abmelden",
                        onclick: move |_| {
                            spawn(async move {
                                let _: Result<nrz_gp_common::Ok, _> = api::post_empty("/api/auth/logout").await;
                                crate::store::on_unauthorized();
                            });
                        },
                    }
                    AppVersion { text: version }
                }
            },
            main { id: "app-main", class: "min-h-0 flex-1 overflow-y-auto",
                Outlet::<Route> {}
            }
        }
        Toaster {}
    }
}

fn route_server_id(r: &Route) -> Option<String> {
    let s = r.to_string();
    let rest = s.strip_prefix("/servers/")?;
    Some(rest.split('/').next()?.to_string())
}

/* ------------------------------------------------------------------ */
/*                           Server-Bereich                           */
/* ------------------------------------------------------------------ */

/// Live-Daten eines Servers für alle Unterseiten (per `use_context::<ServerCtx>()`).
#[derive(Clone, Copy, PartialEq)]
pub struct ServerCtx {
    pub id: Signal<String>,
    pub summary: Signal<Option<ServerSummary>>,
    pub console: Signal<Vec<ConsoleLine>>,
    pub status: Signal<ServerStatus>,
    pub stats: Signal<ServerStats>,
    pub players: Signal<Vec<String>>,
    pub events: Signal<Vec<GameEvent>>,
    /// Befehle an den WebSocket.
    pub cmd_tx: Signal<Option<futures::channel::mpsc::UnboundedSender<String>>>,
}

impl ServerCtx {
    pub fn id(&self) -> String {
        self.id.read().clone()
    }
    /// Konsolenbefehl senden (über den WebSocket, Antwort erscheint in der Konsole).
    pub fn send(&self, cmd: &str) {
        if let Some(tx) = self.cmd_tx.read().as_ref() {
            let _ = tx.unbounded_send(cmd.to_string());
        }
    }
    /// Serverdaten neu laden (nach Änderungen).
    pub fn reload(&self) {
        let mut summary = self.summary;
        let id = self.id();
        spawn(async move {
            if let Ok(s) = api::get::<ServerSummary>(&routes::server(&id, "")).await {
                summary.set(Some(s));
            }
            refresh_servers().await;
        });
    }
    pub fn game(&self) -> Game {
        self.summary.read().as_ref().map_or(Game::Minecraft, |s| s.config.game)
    }
}

pub fn use_server() -> ServerCtx {
    use_context::<ServerCtx>()
}

/// Steuerung (Start/Stopp/Neustart/Kill) für einen Server.
pub async fn power(id: &str, action: PowerAction) {
    let r: Result<nrz_gp_common::Ok, _> = api::post(&routes::server(id, "power"), &PowerRequest { action }).await;
    match r {
        Ok(_) => toast(match action {
            PowerAction::Start => "Server wird gestartet",
            PowerAction::Stop => "Server wird gestoppt",
            PowerAction::Restart => "Server wird neu gestartet",
            PowerAction::Kill => "Server wurde beendet",
        }),
        Err(e) => toast_err(e),
    }
    refresh_servers().await;
}

#[component]
pub fn ServerLayout(id: String) -> Element {
    let mut id_sig = use_signal(|| id.clone());
    if *id_sig.peek() != id {
        id_sig.set(id.clone());
    }
    let mut summary = use_signal(|| crate::store::server(&id));
    let mut console = use_signal(Vec::<ConsoleLine>::new);
    let mut status = use_signal(|| summary.peek().as_ref().map_or(ServerStatus::Stopped, |s| s.status));
    let mut stats = use_signal(ServerStats::default);
    let mut players = use_signal(Vec::<String>::new);
    let mut events = use_signal(Vec::<GameEvent>::new);
    let mut cmd_tx = use_signal(|| None);
    let ctx = ServerCtx { id: id_sig, summary, console, status, stats, players, events, cmd_tx };
    use_context_provider(|| ctx);

    // Zusammenfassung + Ereignisse laden, wenn sich die ID ändert
    use_effect(move || {
        let id = id_sig();
        spawn(async move {
            match api::get::<ServerSummary>(&routes::server(&id, "")).await {
                Ok(s) => {
                    status.set(s.status);
                    summary.set(Some(s));
                }
                Err(e) => toast_err(e),
            }
            if let Ok(ev) = api::get::<Vec<GameEvent>>(&format!("{}?limit=100", routes::server(&id, "events"))).await {
                events.set(ev);
            }
        });
    });

    // WebSocket (Konsole, Status, Statistik, Ereignisse) – neu verbinden bei Abbruch
    use_effect(move || {
        let id = id_sig();
        console.set(Vec::new());
        spawn(async move {
            loop {
                if *id_sig.peek() != id {
                    break;
                }
                let url = api::ws_url(&routes::server(&id, "ws"));
                let Ok(ws) = gloo_net::websocket::futures::WebSocket::open(&url) else {
                    gloo_timers::future::TimeoutFuture::new(3000).await;
                    continue;
                };
                let (mut sink, mut stream) = ws.split();
                let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
                cmd_tx.set(Some(tx));
                let writer = async move {
                    while let Some(cmd) = rx.next().await {
                        let msg = serde_json::to_string(&WsClientMessage::Command { command: cmd }).unwrap_or_default();
                        if sink.send(gloo_net::websocket::Message::Text(msg)).await.is_err() {
                            break;
                        }
                    }
                };
                let rid = id.clone();
                let reader = async move {
                    while let Some(Ok(msg)) = stream.next().await {
                        if *id_sig.peek() != rid {
                            break;
                        }
                        let gloo_net::websocket::Message::Text(t) = msg else { continue };
                        let Ok(m) = serde_json::from_str::<WsMessage>(&t) else { continue };
                        match m {
                            WsMessage::History { lines } => console.set(lines),
                            WsMessage::Line { line } => {
                                let mut c = console.write();
                                c.push(line);
                                let n = c.len();
                                if n > 3000 {
                                    c.drain(0..n - 3000);
                                }
                            }
                            WsMessage::Status { status: s } => {
                                status.set(s);
                                if let Some(sum) = summary.write().as_mut() {
                                    sum.status = s;
                                }
                            }
                            WsMessage::Stats { stats: st, players: p } => {
                                stats.set(st);
                                players.set(p);
                            }
                            WsMessage::Event { event } => {
                                let mut e = events.write();
                                e.insert(0, event);
                                e.truncate(200);
                            }
                        }
                    }
                };
                futures::pin_mut!(writer, reader);
                futures::future::select(writer, reader).await;
                cmd_tx.set(None);
                gloo_timers::future::TimeoutFuture::new(2000).await;
            }
        });
    });

    let route = use_route::<Route>();
    let Some(sum) = summary.read().clone() else {
        return rsx! { crate::components::Loading {} };
    };
    let st = status();
    let host = SYSTEM.read().as_ref().map(|s| s.public_host.clone()).unwrap_or_else(|| "localhost".into());
    let address = if sum.config.port == sum.config.game.default_port() { host.clone() } else { format!("{host}:{}", sum.config.port) };
    let is_mc = sum.config.game == Game::Minecraft;
    let id2 = id.clone();
    let id3 = id.clone();
    let id4 = id.clone();
    let id5 = id.clone();

    let mut tabs: Vec<(Route, &str, Element)> = vec![
        (Route::ServerOverview { id: id.clone() }, "Übersicht", rsx! { icons::LayoutDashboard { class: "size-4" } }),
        (Route::ServerConsole { id: id.clone() }, "Konsole", rsx! { icons::SquareTerminal { class: "size-4" } }),
        (Route::ServerFiles { id: id.clone() }, "Dateien", rsx! { icons::Folder { class: "size-4" } }),
    ];
    if is_mc {
        tabs.push((Route::ServerPlayers { id: id.clone() }, "Spieler", rsx! { icons::Users { class: "size-4" } }));
        tabs.push((Route::ServerMap { id: id.clone() }, "Karte", rsx! { icons::Map { class: "size-4" } }));
        tabs.push((
            Route::ServerAddons { id: id.clone() },
            if sum.config.flavor.uses_mods() { "Mods" } else { "Plugins" },
            rsx! { icons::Puzzle { class: "size-4" } },
        ));
    } else {
        tabs.push((Route::FactorioPage { id: id.clone() }, "Factorio", rsx! { icons::Factory { class: "size-4" } }));
    }
    tabs.push((Route::ServerBackups { id: id.clone() }, "Backups", rsx! { icons::Archive { class: "size-4" } }));
    tabs.push((Route::ServerAutomations { id: id.clone() }, "Automationen", rsx! { icons::Zap { class: "size-4" } }));
    tabs.push((Route::ServerSettings { id: id.clone() }, "Einstellungen", rsx! { icons::Settings2 { class: "size-4" } }));

    let route_str = route.to_string();

    rsx! {
        document::Title { "{sum.config.name} · nrzGamePanel" }
        div { class: "border-b bg-background/95",
            div { class: "mx-auto flex max-w-7xl flex-wrap items-center gap-3 px-4 pt-5 md:px-8",
                div { class: "flex size-10 items-center justify-center rounded-lg bg-muted",
                    if is_mc {
                        icons::Pickaxe { class: "size-5" }
                    } else {
                        icons::Factory { class: "size-5" }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        h1 { class: "truncate text-xl font-semibold tracking-tight", "{sum.config.name}" }
                        StatusBadge { status: st }
                    }
                    div { class: "mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground",
                        span { "{sum.config.flavor.label()} {sum.config.version.clone().unwrap_or_default()}" }
                        button {
                            class: "inline-flex items-center gap-1 font-mono hover:text-foreground",
                            title: "Adresse kopieren",
                            onclick: {
                                let a = address.clone();
                                move |_| {
                                    api::copy(&a);
                                    toast("Adresse kopiert");
                                }
                            },
                            icons::Copy { class: "size-3" }
                            "{address}"
                        }
                        if st.is_online() {
                            span { "{players.read().len()} / {stats.read().players_max} Spieler" }
                        }
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    if st == ServerStatus::Stopped || st == ServerStatus::Crashed {
                        Button {
                            size: ButtonSize::Sm,
                            onclick: move |_| {
                                let id = id2.clone();
                                spawn(async move { power(&id, PowerAction::Start).await });
                            },
                            icons::Play { class: "size-4" }
                            "Starten"
                        }
                    }
                    if st.is_online() {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            disabled: st == ServerStatus::Stopping,
                            onclick: move |_| {
                                let id = id3.clone();
                                spawn(async move { power(&id, PowerAction::Restart).await });
                            },
                            icons::RotateCw { class: "size-4" }
                            "Neustart"
                        }
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            disabled: st == ServerStatus::Stopping,
                            onclick: move |_| {
                                let id = id4.clone();
                                spawn(async move { power(&id, PowerAction::Stop).await });
                            },
                            icons::Square { class: "size-4" }
                            "Stoppen"
                        }
                    }
                    if st == ServerStatus::Stopping {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Destructive,
                            onclick: move |_| {
                                let id = id5.clone();
                                spawn(async move { power(&id, PowerAction::Kill).await });
                            },
                            icons::Skull { class: "size-4" }
                            "Beenden"
                        }
                    }
                }
            }
            nav { class: "mx-auto mt-3 flex max-w-7xl gap-1 overflow-x-auto px-2 md:px-6",
                for (r, label, icon) in tabs {
                    {
                        let rs = r.to_string();
                        let active = if rs.ends_with(&format!("/servers/{id}")) || rs.ends_with(&format!("/servers/{id}/")) {
                            route_str == rs || route_str.trim_end_matches('/') == rs.trim_end_matches('/')
                        } else {
                            route_str.starts_with(&rs)
                        };
                        rsx! {
                            Link {
                                key: "{rs}",
                                to: r,
                                class: if active {
                                    "flex shrink-0 items-center gap-1.5 border-b-2 border-foreground px-3 pb-2.5 pt-1 text-sm font-medium"
                                } else {
                                    "flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-3 pb-2.5 pt-1 text-sm text-muted-foreground hover:text-foreground"
                                },
                                {icon}
                                "{label}"
                            }
                        }
                    }
                }
            }
        }
        Outlet::<Route> {}
    }
}
