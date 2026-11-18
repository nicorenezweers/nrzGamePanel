//! Spieler: Übersicht (online, alle bekannten, Whitelist, OPs, Bans) und Spielerseite mit
//! Werten, Aktionen, Inventar und Endertruhe (Drag & Drop, Items setzen/ändern/leeren).

use dioxus::prelude::*;
use nrz_gp_common::{
    CommandResponse, Container, InventoryOp, ItemCatalog, ItemStack, KnownPlayer, PlayerAction, PlayerDetail, PlayersOverview,
    ToggleRequest, item_display_name, routes, slot_label,
};

use crate::{
    Route, api,
    components::{
        ConfirmDialog, Dropdown, ErrorBox, Field, ICON_BUTTON, INPUT, Loading, MENU_ITEM, MENU_ITEM_DANGER, MenuLabel, MenuSeparator, Modal,
        Notice, PlayerHead, SELECT, Segmented, Toggle, fmt_ago, fmt_datetime,
        ui::{
            app_shell::EmptyState,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::use_server,
    store::{toast, toast_err},
};

/* ------------------------------------------------------------------ */
/*                              Helfer                                */
/* ------------------------------------------------------------------ */

const GAMEMODES: [(&str, &str); 4] = [("survival", "Überleben"), ("creative", "Kreativ"), ("adventure", "Abenteuer"), ("spectator", "Zuschauer")];

fn gamemode_label(m: &str) -> String {
    GAMEMODES.iter().find(|(k, _)| *k == m).map_or_else(|| m.to_string(), |(_, l)| (*l).to_string())
}

pub(crate) fn dimension_label(d: &str) -> String {
    match d {
        "minecraft:overworld" | "overworld" => "Oberwelt".into(),
        "minecraft:the_nether" | "the_nether" => "Nether".into(),
        "minecraft:the_end" | "the_end" => "End".into(),
        other => item_display_name(other),
    }
}

fn roman(n: u32) -> String {
    match n {
        1 => "I".into(),
        2 => "II".into(),
        3 => "III".into(),
        4 => "IV".into(),
        5 => "V".into(),
        6 => "VI".into(),
        7 => "VII".into(),
        8 => "VIII".into(),
        9 => "IX".into(),
        10 => "X".into(),
        n => n.to_string(),
    }
}

/// Spieleraktion ausführen (Name, nicht UUID).
pub(crate) async fn player_action(id: &str, name: &str, action: &PlayerAction) -> Result<CommandResponse, String> {
    api::post::<_, CommandResponse>(&routes::server(id, &format!("mc/players/{}/action", routes::enc(name))), action).await
}

fn action_done_msg(action: &PlayerAction, name: &str) -> String {
    match action {
        PlayerAction::Kick { .. } => format!("{name} wurde gekickt"),
        PlayerAction::Ban { .. } => format!("{name} wurde gebannt"),
        PlayerAction::Pardon => format!("{name} wurde entbannt"),
        PlayerAction::Op => format!("{name} ist jetzt OP"),
        PlayerAction::Deop => format!("{name} ist kein OP mehr"),
        PlayerAction::WhitelistAdd => format!("{name} ist auf der Whitelist"),
        PlayerAction::WhitelistRemove => format!("{name} wurde von der Whitelist entfernt"),
        PlayerAction::Gamemode { mode } => format!("Spielmodus von {name}: {}", gamemode_label(mode)),
        PlayerAction::Teleport { .. } | PlayerAction::TeleportTo { .. } => format!("{name} wurde teleportiert"),
        PlayerAction::Heal => format!("{name} wurde geheilt"),
        PlayerAction::Feed => format!("{name} wurde gesättigt"),
        PlayerAction::Kill => format!("{name} wurde getötet"),
        PlayerAction::Message { .. } => format!("Nachricht an {name} gesendet"),
    }
}

/// Aktion mit Meldung ausführen; `true` bei Erfolg.
async fn run_action(id: &str, name: &str, action: PlayerAction) -> bool {
    match player_action(id, name, &action).await {
        Ok(r) => {
            let out = r.output.unwrap_or_default();
            let out = out.trim();
            if out.is_empty() || out.len() > 160 { toast(action_done_msg(&action, name)) } else { toast(out.to_string()) }
            true
        }
        Err(e) => {
            toast_err(e);
            false
        }
    }
}

/// Dialog mit optionalem Grund (Kick/Ban).
#[component]
fn ReasonDialog(
    #[props(into)] title: String,
    #[props(into)] description: String,
    #[props(into)] confirm_label: String,
    onsubmit: EventHandler<Option<String>>,
    onclose: EventHandler<()>,
) -> Element {
    let mut reason = use_signal(String::new);
    let submit = move || {
        let r = reason().trim().to_string();
        onsubmit.call(if r.is_empty() { None } else { Some(r) });
    };
    rsx! {
        Modal {
            title,
            description,
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button { variant: ButtonVariant::Destructive, onclick: move |_| submit(), "{confirm_label}" }
            },
            form {
                onsubmit: move |e| {
                    e.prevent_default();
                    submit();
                },
                Field { label: "Grund (optional)", hint: "Wird dem Spieler angezeigt.",
                    input {
                        class: INPUT,
                        value: "{reason}",
                        placeholder: "z. B. Regelverstoss",
                        onmounted: move |e| {
                            spawn(async move {
                                let _ = e.data().set_focus(true).await;
                            });
                        },
                        oninput: move |e| reason.set(e.value()),
                    }
                }
            }
        }
    }
}

/// Kleines Abzeichen.
#[component]
fn Pill(#[props(into)] text: String, #[props(into, default = "bg-muted text-muted-foreground".to_string())] tone: String) -> Element {
    rsx! { span { class: "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium {tone}", "{text}" } }
}

/* ------------------------------------------------------------------ */
/*                          Spieler-Übersicht                         */
/* ------------------------------------------------------------------ */

#[derive(Clone, PartialEq)]
enum ListDialog {
    None,
    Kick(String),
    Ban(String),
}

#[component]
pub fn ServerPlayers(id: String) -> Element {
    let ctx = use_server();
    let mut reload = use_signal(|| 0u32);
    let mut search = use_signal(String::new);
    let mut filter = use_signal(|| "all".to_string());
    let mut dialog = use_signal(|| ListDialog::None);
    let mut wl_name = use_signal(String::new);
    let mut op_name = use_signal(String::new);
    let mut wl_busy = use_signal(|| false);

    let overview = use_resource(move || {
        let id = ctx.id();
        let _ = reload();
        let _ = ctx.players.read().len();
        async move { api::get::<PlayersOverview>(&routes::server(&id, "mc/players")).await }
    });

    // Aktion ausführen und danach neu laden.
    let act = move |name: String, action: PlayerAction| {
        let id = ctx.id();
        spawn(async move {
            if run_action(&id, &name, action).await {
                gloo_timers::future::TimeoutFuture::new(400).await;
                reload += 1;
            }
        });
    };

    let state = overview.read().clone();
    let ov = match state {
        None => return rsx! { div { class: "mx-auto max-w-7xl p-4 md:p-8", Loading { text: "Lade Spieler …" } } },
        Some(Err(e)) => {
            return rsx! {
                div { class: "mx-auto flex max-w-7xl flex-col items-start gap-3 p-4 md:p-8",
                    ErrorBox { message: e }
                    Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, onclick: move |_| reload += 1, "Erneut versuchen" }
                }
            };
        }
        Some(Ok(o)) => o,
    };

    let server_online = ctx.status.read().is_online();
    let live_names: Vec<String> = ctx.players.read().clone();
    let mut online: Vec<(Option<String>, String)> = ov.online.iter().map(|p| (p.uuid.clone(), p.name.clone())).collect();
    for n in &live_names {
        if !online.iter().any(|(_, m)| m.eq_ignore_ascii_case(n)) {
            online.push((None, n.clone()));
        }
    }
    let is_online = |name: &str, k: &KnownPlayer| k.online || online.iter().any(|(_, n)| n.eq_ignore_ascii_case(name));

    let q = search().to_lowercase();
    let fl = filter();
    let mut players: Vec<KnownPlayer> = ov
        .players
        .iter()
        .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.uuid.to_lowercase().contains(&q))
        .filter(|p| match fl.as_str() {
            "online" => is_online(&p.name, p),
            "op" => p.op,
            "whitelist" => p.whitelisted,
            "banned" => p.banned,
            _ => true,
        })
        .cloned()
        .collect();
    players.sort_by(|a, b| {
        is_online(&b.name, b).cmp(&is_online(&a.name, a)).then(b.last_seen.cmp(&a.last_seen)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let counts = (
        ov.players.len(),
        ov.players.iter().filter(|p| is_online(&p.name, p)).count(),
        ov.players.iter().filter(|p| p.op).count(),
        ov.players.iter().filter(|p| p.whitelisted).count(),
        ov.players.iter().filter(|p| p.banned).count(),
    );
    let filters = vec![
        ("all".to_string(), format!("Alle {}", counts.0)),
        ("online".to_string(), format!("Online {}", counts.1)),
        ("op".to_string(), format!("OP {}", counts.2)),
        ("whitelist".to_string(), format!("Whitelist {}", counts.3)),
        ("banned".to_string(), format!("Gebannt {}", counts.4)),
    ];
    let wl_enabled = ov.whitelist_enabled;

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-6 p-4 md:p-8",
            if !ov.live {
                Notice {
                    if server_online {
                        "Keine Live-Verbindung (RCON). Spielerdaten stammen aus den Dateien; manche Aktionen sind erst nach einem Neustart wirksam."
                    } else {
                        "Der Server ist gestoppt. Du siehst die gespeicherten Daten; Kick, Spielmodus & Co. gehen nur bei laufendem Server."
                    }
                }
            }
            div { class: "grid gap-6 lg:grid-cols-3",
                div { class: "flex min-w-0 flex-col gap-6 lg:col-span-2",
                    // Online
                    section {
                        div { class: "mb-3 flex items-center justify-between",
                            h2 { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground",
                                "Online · {online.len()} / {ov.max_players}"
                            }
                            button { class: ICON_BUTTON, title: "Aktualisieren", aria_label: "Aktualisieren", onclick: move |_| reload += 1,
                                icons::RefreshCw { class: "size-4" }
                            }
                        }
                        if online.is_empty() {
                            div { class: "rounded-xl border border-dashed px-4 py-8 text-center text-sm text-muted-foreground",
                                if server_online { "Gerade ist niemand online." } else { "Server ist gestoppt – niemand online." }
                            }
                        } else {
                            div { class: "grid gap-3 sm:grid-cols-2 xl:grid-cols-3",
                                for (uuid, name) in online.iter().cloned() {
                                    {
                                        let k = ov.players.iter().find(|p| p.name.eq_ignore_ascii_case(&name)).cloned();
                                        let target = uuid.clone().or_else(|| k.as_ref().map(|k| k.uuid.clone())).unwrap_or_else(|| name.clone());
                                        let op = k.as_ref().is_some_and(|k| k.op);
                                        let n1 = name.clone();
                                        let n2 = name.clone();
                                        let n3 = name.clone();
                                        rsx! {
                                            div { key: "{name}", class: "flex items-center gap-3 rounded-xl border bg-card p-3 shadow-sm",
                                                Link { to: Route::PlayerPage { id: id.clone(), uuid: target.clone() }, class: "flex min-w-0 flex-1 items-center gap-3",
                                                    PlayerHead { player: name.clone(), size: 10 }
                                                    div { class: "min-w-0",
                                                        div { class: "flex items-center gap-1.5",
                                                            span { class: "size-2 shrink-0 rounded-full bg-success" }
                                                            span { class: "truncate text-sm font-medium", "{name}" }
                                                        }
                                                        if op {
                                                            span { class: "text-xs text-muted-foreground", "Operator" }
                                                        } else {
                                                            span { class: "text-xs text-muted-foreground", "Spieler" }
                                                        }
                                                    }
                                                }
                                                Link {
                                                    to: Route::PlayerPage { id: id.clone(), uuid: target.clone() },
                                                    class: ICON_BUTTON,
                                                    title: "Inventar",
                                                    icons::Backpack { class: "size-4" }
                                                }
                                                Dropdown { label: "Aktionen für {name}",
                                                    MenuLabel { text: "Spielmodus" }
                                                    for (mode, label) in GAMEMODES {
                                                        button {
                                                            key: "{mode}",
                                                            class: MENU_ITEM,
                                                            onclick: {
                                                                let n = name.clone();
                                                                move |_| act(n.clone(), PlayerAction::Gamemode { mode: mode.to_string() })
                                                            },
                                                            icons::Gamepad2 {}
                                                            "{label}"
                                                        }
                                                    }
                                                    MenuSeparator {}
                                                    button {
                                                        class: MENU_ITEM,
                                                        onclick: move |_| act(n1.clone(), if op { PlayerAction::Deop } else { PlayerAction::Op }),
                                                        icons::Crown {}
                                                        if op { "OP entziehen" } else { "Zum OP machen" }
                                                    }
                                                    button { class: MENU_ITEM, onclick: move |_| act(n2.clone(), PlayerAction::Heal),
                                                        icons::Heart {}
                                                        "Heilen"
                                                    }
                                                    MenuSeparator {}
                                                    button { class: MENU_ITEM_DANGER, onclick: move |_| dialog.set(ListDialog::Kick(n3.clone())),
                                                        icons::LogOut {}
                                                        "Kicken …"
                                                    }
                                                    button {
                                                        class: MENU_ITEM_DANGER,
                                                        onclick: {
                                                            let n = name.clone();
                                                            move |_| dialog.set(ListDialog::Ban(n.clone()))
                                                        },
                                                        icons::Ban {}
                                                        "Bannen …"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Alle bekannten Spieler
                    section { class: "flex flex-col gap-3",
                        h2 { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "Alle bekannten Spieler" }
                        div { class: "flex flex-wrap items-center gap-2",
                            div { class: "relative min-w-48 flex-1",
                                icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                                input {
                                    class: "{INPUT} pl-8",
                                    placeholder: "Name oder UUID suchen",
                                    value: "{search}",
                                    oninput: move |e| search.set(e.value()),
                                }
                            }
                            Segmented { items: filters, active: fl.clone(), onchange: move |v| filter.set(v) }
                        }
                        if ov.players.is_empty() {
                            EmptyState {
                                title: "Noch keine Spieler",
                                description: "Sobald jemand den Server betritt, erscheint er hier – mit Inventar, Position und Werten.",
                                icon: rsx! { icons::Users {} },
                            }
                        } else if players.is_empty() {
                            div { class: "rounded-xl border border-dashed px-4 py-8 text-center text-sm text-muted-foreground",
                                "Kein Spieler passt zu Suche und Filter."
                            }
                        } else {
                            div { class: "rounded-xl border bg-card shadow-sm",
                                for p in players.iter().cloned() {
                                    {
                                        let on = is_online(&p.name, &p);
                                        rsx! {
                                            KnownPlayerRow {
                                                key: "{p.uuid}",
                                                server_id: id.clone(),
                                                player: p.clone(),
                                                online: on,
                                                onaction: move |(n, a): (String, PlayerAction)| act(n, a),
                                                onkick: move |n: String| dialog.set(ListDialog::Kick(n)),
                                                onban: move |n: String| dialog.set(ListDialog::Ban(n)),
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Seitenspalte: Whitelist, OPs, Bans
                div { class: "flex min-w-0 flex-col gap-6",
                    div { class: "rounded-xl border bg-card shadow-sm",
                        div { class: "flex items-center justify-between gap-3 border-b px-4 py-3",
                            div {
                                h3 { class: "text-sm font-semibold", "Whitelist" }
                                p { class: "text-xs text-muted-foreground",
                                    if wl_enabled { "Nur eingetragene Spieler dürfen beitreten." } else { "Aus – alle dürfen beitreten." }
                                }
                            }
                            Toggle {
                                checked: wl_enabled,
                                disabled: wl_busy(),
                                label: "Whitelist",
                                onchange: move |v: bool| {
                                    let id = ctx.id();
                                    wl_busy.set(true);
                                    spawn(async move {
                                        match api::post::<_, nrz_gp_common::Ok>(&routes::server(&id, "mc/whitelist"), &ToggleRequest { enabled: v }).await {
                                            Ok(_) => toast(if v { "Whitelist eingeschaltet" } else { "Whitelist ausgeschaltet" }),
                                            Err(e) => toast_err(e),
                                        }
                                        wl_busy.set(false);
                                        reload += 1;
                                    });
                                },
                            }
                        }
                        form {
                            class: "flex gap-2 border-b px-4 py-3",
                            onsubmit: move |e| {
                                e.prevent_default();
                                let n = wl_name().trim().to_string();
                                if !n.is_empty() {
                                    wl_name.set(String::new());
                                    act(n, PlayerAction::WhitelistAdd);
                                }
                            },
                            input {
                                class: "{INPUT} h-8",
                                placeholder: "Spielername",
                                value: "{wl_name}",
                                autocomplete: "off",
                                oninput: move |e| wl_name.set(e.value()),
                            }
                            Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, button_type: "submit", disabled: wl_name().trim().is_empty(),
                                icons::UserPlus { class: "size-4" }
                                "Hinzufügen"
                            }
                        }
                        NameList {
                            names: ov.whitelist.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                            empty: "Niemand auf der Whitelist.",
                            remove_label: "Von der Whitelist entfernen",
                            onremove: move |n: String| act(n, PlayerAction::WhitelistRemove),
                        }
                    }

                    div { class: "rounded-xl border bg-card shadow-sm",
                        div { class: "border-b px-4 py-3",
                            h3 { class: "text-sm font-semibold", "Operatoren" }
                            p { class: "text-xs text-muted-foreground", "OPs dürfen alle Befehle im Spiel nutzen." }
                        }
                        form {
                            class: "flex gap-2 border-b px-4 py-3",
                            onsubmit: move |e| {
                                e.prevent_default();
                                let n = op_name().trim().to_string();
                                if !n.is_empty() {
                                    op_name.set(String::new());
                                    act(n, PlayerAction::Op);
                                }
                            },
                            input {
                                class: "{INPUT} h-8",
                                placeholder: "Spielername",
                                value: "{op_name}",
                                autocomplete: "off",
                                oninput: move |e| op_name.set(e.value()),
                            }
                            Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, button_type: "submit", disabled: op_name().trim().is_empty(),
                                icons::Crown { class: "size-4" }
                                "OP"
                            }
                        }
                        NameList {
                            names: ov.ops.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                            empty: "Keine Operatoren.",
                            remove_label: "OP entziehen",
                            onremove: move |n: String| act(n, PlayerAction::Deop),
                        }
                    }

                    div { class: "rounded-xl border bg-card shadow-sm",
                        div { class: "border-b px-4 py-3",
                            h3 { class: "text-sm font-semibold", "Bannliste" }
                            p { class: "text-xs text-muted-foreground", "Gebannte Spieler und IP-Adressen." }
                        }
                        if ov.banned_players.is_empty() && ov.banned_ips.is_empty() {
                            p { class: "px-4 py-6 text-center text-sm text-muted-foreground", "Niemand ist gebannt." }
                        }
                        div { class: "divide-y",
                            for b in ov.banned_players.iter().cloned() {
                                div { key: "p{b.name}", class: "flex items-start gap-3 px-4 py-2.5",
                                    PlayerHead { player: b.name.clone(), size: 6 }
                                    div { class: "min-w-0 flex-1",
                                        p { class: "truncate text-sm font-medium", "{b.name}" }
                                        p { class: "text-xs text-muted-foreground",
                                            "{b.reason.clone().unwrap_or_else(|| \"Kein Grund angegeben\".into())}"
                                        }
                                        if let Some(exp) = b.expires.clone().filter(|e| e != "forever") {
                                            p { class: "text-xs text-muted-foreground", "Bis {exp}" }
                                        }
                                    }
                                    Button {
                                        size: ButtonSize::Sm,
                                        variant: ButtonVariant::Outline,
                                        onclick: {
                                            let n = b.name.clone();
                                            move |_| act(n.clone(), PlayerAction::Pardon)
                                        },
                                        "Entbannen"
                                    }
                                }
                            }
                            for b in ov.banned_ips.iter().cloned() {
                                div { key: "ip{b.name}", class: "flex items-start gap-3 px-4 py-2.5",
                                    div { class: "flex size-6 items-center justify-center rounded-sm bg-muted", icons::Globe { class: "size-3.5 text-muted-foreground" } }
                                    div { class: "min-w-0 flex-1",
                                        p { class: "truncate font-mono text-sm", "{b.name}" }
                                        p { class: "text-xs text-muted-foreground",
                                            "{b.reason.clone().unwrap_or_else(|| \"IP-Bann\".into())}"
                                        }
                                    }
                                    Button {
                                        size: ButtonSize::Sm,
                                        variant: ButtonVariant::Outline,
                                        disabled: !server_online,
                                        title: if server_online { "IP entbannen" } else { "Nur bei laufendem Server" },
                                        onclick: {
                                            let ip = b.name.clone();
                                            move |_| {
                                                ctx.send(&format!("pardon-ip {ip}"));
                                                toast(format!("{ip} wird entbannt"));
                                                spawn(async move {
                                                    gloo_timers::future::TimeoutFuture::new(800).await;
                                                    reload += 1;
                                                });
                                            }
                                        },
                                        "Entbannen"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        match dialog() {
            ListDialog::None => rsx! {},
            ListDialog::Kick(name) => rsx! {
                ReasonDialog {
                    title: "{name} kicken",
                    description: "Der Spieler wird vom Server getrennt und kann sofort wieder beitreten.",
                    confirm_label: "Kicken",
                    onclose: move |_| dialog.set(ListDialog::None),
                    onsubmit: move |reason| {
                        dialog.set(ListDialog::None);
                        act(name.clone(), PlayerAction::Kick { reason });
                    },
                }
            },
            ListDialog::Ban(name) => rsx! {
                ReasonDialog {
                    title: "{name} bannen",
                    description: "Der Spieler wird getrennt und kann nicht mehr beitreten, bis du ihn entbannst.",
                    confirm_label: "Bannen",
                    onclose: move |_| dialog.set(ListDialog::None),
                    onsubmit: move |reason| {
                        dialog.set(ListDialog::None);
                        act(name.clone(), PlayerAction::Ban { reason });
                    },
                }
            },
        }
    }
}

#[component]
fn NameList(names: Vec<String>, #[props(into)] empty: String, #[props(into)] remove_label: String, onremove: EventHandler<String>) -> Element {
    rsx! {
        if names.is_empty() {
            p { class: "px-4 py-6 text-center text-sm text-muted-foreground", "{empty}" }
        } else {
            div { class: "flex max-h-72 flex-col overflow-y-auto py-1",
                for n in names {
                    div { key: "{n}", class: "group flex items-center gap-2.5 px-4 py-1.5 hover:bg-accent/40",
                        PlayerHead { player: n.clone(), size: 5 }
                        span { class: "min-w-0 flex-1 truncate text-sm", "{n}" }
                        button {
                            class: "{ICON_BUTTON} size-7",
                            title: "{remove_label}",
                            aria_label: "{remove_label}: {n}",
                            onclick: {
                                let n = n.clone();
                                move |_| onremove.call(n.clone())
                            },
                            icons::X { class: "size-3.5" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn KnownPlayerRow(
    server_id: String,
    player: KnownPlayer,
    online: bool,
    onaction: EventHandler<(String, PlayerAction)>,
    onkick: EventHandler<String>,
    onban: EventHandler<String>,
) -> Element {
    let p = player;
    let name = p.name.clone();
    let n = name.clone();
    let a = move |action: PlayerAction| onaction.call((n.clone(), action));
    let nk = name.clone();
    let nb = name.clone();
    rsx! {
        div { class: "flex items-center gap-3 border-b px-3 py-2 last:border-b-0 hover:bg-accent/30",
            Link { to: Route::PlayerPage { id: server_id.clone(), uuid: p.uuid.clone() }, class: "flex min-w-0 flex-1 items-center gap-3",
                div { class: "relative",
                    PlayerHead { player: name.clone(), size: 8 }
                    if online {
                        span { class: "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-card bg-success" }
                    }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-1.5",
                        span { class: "truncate text-sm font-medium", "{name}" }
                        if online { Pill { text: "Online", tone: "bg-success/15 text-success" } }
                        if p.op { Pill { text: "OP", tone: "bg-info/15 text-info" } }
                        if p.whitelisted { Pill { text: "Whitelist" } }
                        if p.banned { Pill { text: "Gebannt", tone: "bg-destructive/15 text-destructive" } }
                    }
                    p { class: "truncate text-xs text-muted-foreground",
                        if online {
                            "Gerade online"
                        } else if let Some(t) = p.last_seen {
                            span { title: "{fmt_datetime(t)}", "Zuletzt gesehen {fmt_ago(t)}" }
                        } else {
                            "Noch nie gesehen"
                        }
                    }
                }
            }
            Dropdown { label: "Aktionen für {name}",
                if online {
                    MenuLabel { text: "Spielmodus" }
                    for (mode, label) in GAMEMODES {
                        button {
                            key: "{mode}",
                            class: MENU_ITEM,
                            onclick: {
                                let a = a.clone();
                                move |_| a(PlayerAction::Gamemode { mode: mode.to_string() })
                            },
                            icons::Gamepad2 {}
                            "{label}"
                        }
                    }
                    MenuSeparator {}
                }
                button {
                    class: MENU_ITEM,
                    onclick: {
                        let a = a.clone();
                        let op = p.op;
                        move |_| a(if op { PlayerAction::Deop } else { PlayerAction::Op })
                    },
                    icons::Crown {}
                    if p.op { "OP entziehen" } else { "Zum OP machen" }
                }
                button {
                    class: MENU_ITEM,
                    onclick: {
                        let a = a.clone();
                        let wl = p.whitelisted;
                        move |_| a(if wl { PlayerAction::WhitelistRemove } else { PlayerAction::WhitelistAdd })
                    },
                    icons::ListChecks {}
                    if p.whitelisted { "Von Whitelist entfernen" } else { "Auf Whitelist setzen" }
                }
                MenuSeparator {}
                if online {
                    button { class: MENU_ITEM_DANGER, onclick: move |_| onkick.call(nk.clone()),
                        icons::LogOut {}
                        "Kicken …"
                    }
                }
                if p.banned {
                    button {
                        class: MENU_ITEM,
                        onclick: {
                            let a = a.clone();
                            move |_| a(PlayerAction::Pardon)
                        },
                        icons::ShieldCheck {}
                        "Entbannen"
                    }
                } else {
                    button { class: MENU_ITEM_DANGER, onclick: move |_| onban.call(nb.clone()),
                        icons::Ban {}
                        "Bannen …"
                    }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                             Spielerseite                           */
/* ------------------------------------------------------------------ */

type SlotRef = (Container, i32);

#[derive(Clone, PartialEq)]
enum PDialog {
    None,
    Slot(SlotRef),
    Give,
    ClearAll(Container),
    Teleport,
    Message,
    Kick,
    Ban,
}

/// Item-Katalog je Server (einmal geladen).
static ITEM_CATALOG: GlobalSignal<Option<(String, Vec<String>)>> = Signal::global(|| None);

fn use_catalog(server_id: String) -> Option<Vec<String>> {
    let sid = server_id.clone();
    use_effect(move || {
        let loaded = ITEM_CATALOG.peek().as_ref().is_some_and(|(s, _)| *s == sid);
        if !loaded {
            let sid = sid.clone();
            spawn(async move {
                match api::get::<ItemCatalog>(&routes::server(&sid, "mc/items")).await {
                    Ok(c) => {
                        let mut items = c.items;
                        items.sort();
                        items.dedup();
                        *ITEM_CATALOG.write() = Some((sid, items));
                    }
                    Err(e) => toast_err(format!("Item-Katalog nicht geladen: {e}")),
                }
            });
        }
    });
    ITEM_CATALOG.read().as_ref().filter(|(s, _)| *s == server_id).map(|(_, v)| v.clone())
}

fn find_item(d: &PlayerDetail, (c, slot): SlotRef) -> Option<ItemStack> {
    let list = match c {
        Container::Inventory => &d.inventory,
        Container::EnderChest => &d.ender_chest,
    };
    list.iter().find(|i| i.slot == slot && !i.id.is_empty() && i.count > 0).cloned()
}

#[component]
pub fn PlayerPage(id: String, uuid: String) -> Element {
    let ctx = use_server();
    let nav = navigator();
    let mut uid = use_signal(|| uuid.clone());
    if *uid.peek() != uuid {
        uid.set(uuid.clone());
    }
    let mut detail = use_signal(|| None::<PlayerDetail>);
    let mut load_err = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut auto = use_signal(|| true);
    let mut dialog = use_signal(|| PDialog::None);
    let drag = use_signal(|| None::<SlotRef>);
    let mut tab = use_signal(|| "inv".to_string());

    let load = use_callback(move |()| {
        let id = ctx.id();
        let u = uid();
        spawn(async move {
            match api::get::<PlayerDetail>(&routes::server(&id, &format!("mc/players/{}", routes::enc(&u)))).await {
                Ok(d) => {
                    if *uid.peek() == u {
                        detail.set(Some(d));
                        load_err.set(None);
                    }
                }
                Err(e) => load_err.set(Some(e)),
            }
        });
    });
    use_effect(move || {
        let _ = uid();
        detail.set(None);
        load_err.set(None);
        load.call(());
    });
    // Auto-Aktualisierung alle 10 s (nur online, nicht während Dialog/Ziehen/Aktion)
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(10_000).await;
            let online = detail.peek().as_ref().is_some_and(|d| d.online);
            if online && *auto.peek() && *dialog.peek() == PDialog::None && drag.peek().is_none() && !*busy.peek() {
                load.call(());
            }
        }
    });

    let inv_op = use_callback(move |op: InventoryOp| {
        let id = ctx.id();
        let u = detail.peek().as_ref().map(|d| d.uuid.clone()).unwrap_or_else(|| uid.peek().clone());
        busy.set(true);
        spawn(async move {
            match api::post::<_, PlayerDetail>(&routes::server(&id, &format!("mc/players/{}/inventory", routes::enc(&u))), &op).await {
                Ok(d) => detail.set(Some(d)),
                Err(e) => toast_err(e),
            }
            busy.set(false);
        });
    });

    let act = use_callback(move |action: PlayerAction| {
        let id = ctx.id();
        let Some(name) = detail.peek().as_ref().map(|d| d.name.clone()) else { return };
        spawn(async move {
            if run_action(&id, &name, action).await {
                gloo_timers::future::TimeoutFuture::new(500).await;
                load.call(());
            }
        });
    });

    let Some(d) = detail() else {
        return rsx! {
            div { class: "mx-auto flex max-w-7xl flex-col gap-4 p-4 md:p-8",
                Link { to: Route::ServerPlayers { id: id.clone() }, class: "inline-flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground",
                    icons::ArrowLeft { class: "size-4" }
                    "Alle Spieler"
                }
                if let Some(e) = load_err() {
                    ErrorBox { message: "Spieler konnte nicht geladen werden: {e}" }
                    Button { size: ButtonSize::Sm, variant: ButtonVariant::Outline, onclick: move |_| load.call(()), "Erneut versuchen" }
                } else {
                    Loading { text: "Lade Spieler …" }
                }
            }
        };
    };

    let online = d.online;
    let live = d.source == "live";
    let other_online: Vec<String> = ctx.players.read().iter().filter(|n| !n.eq_ignore_ascii_case(&d.name)).cloned().collect();

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-6 p-4 md:p-8",
            div { class: "flex flex-wrap items-center justify-between gap-2",
                Link { to: Route::ServerPlayers { id: id.clone() }, class: "inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground",
                    icons::ArrowLeft { class: "size-4" }
                    "Alle Spieler"
                }
                div { class: "flex items-center gap-3 text-xs text-muted-foreground",
                    if online {
                        label { class: "flex items-center gap-2",
                            "Auto-Aktualisierung"
                            Toggle { checked: auto(), label: "Auto-Aktualisierung", onchange: move |v| auto.set(v) }
                        }
                    }
                    button { class: ICON_BUTTON, title: "Neu laden", aria_label: "Neu laden", onclick: move |_| load.call(()),
                        icons::RefreshCw { class: "size-4" }
                    }
                }
            }

            // Kopf
            div { class: "flex flex-col gap-5 rounded-xl border bg-card p-4 shadow-sm sm:flex-row sm:items-center sm:p-6",
                div { class: "relative w-fit",
                    PlayerHead { player: d.name.clone(), size: 20 }
                    span { class: if online { "absolute -right-1 -bottom-1 size-4 rounded-full border-[3px] border-card bg-success" } else { "absolute -right-1 -bottom-1 size-4 rounded-full border-[3px] border-card bg-muted-foreground/50" } }
                }
                div { class: "min-w-0 flex-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        h1 { class: "truncate text-2xl font-semibold tracking-tight", "{d.name}" }
                        if online {
                            Pill { text: "Online", tone: "bg-success/15 text-success" }
                        } else {
                            Pill { text: "Offline" }
                        }
                        if d.op { Pill { text: "OP", tone: "bg-info/15 text-info" } }
                        if d.whitelisted { Pill { text: "Whitelist" } }
                        if d.banned { Pill { text: "Gebannt", tone: "bg-destructive/15 text-destructive" } }
                    }
                    div { class: "mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground",
                        span { class: "inline-flex items-center gap-1",
                            if live { icons::Zap { class: "size-3" } } else { icons::File { class: "size-3" } }
                            if live { "Live-Daten" } else { "Aus Spielerdatei" }
                        }
                        if !online {
                            if let Some(t) = d.last_seen {
                                span { title: "{fmt_datetime(t)}", "Zuletzt gesehen {fmt_ago(t)}" }
                            }
                        }
                        button {
                            class: "inline-flex items-center gap-1 font-mono hover:text-foreground",
                            title: "UUID kopieren",
                            onclick: {
                                let u = d.uuid.clone();
                                move |_| {
                                    api::copy(&u);
                                    toast("UUID kopiert");
                                }
                            },
                            icons::Copy { class: "size-3" }
                            "{d.uuid}"
                        }
                    }
                }
            }

            // Werte + Aktionen
            div { class: "grid gap-6 lg:grid-cols-3",
                div { class: "grid gap-4 rounded-xl border bg-card p-4 shadow-sm sm:grid-cols-2 sm:p-6 lg:col-span-2",
                    StatBlock { label: "Leben",
                        if let Some(h) = d.health {
                            IconBar { value: h, kind: BarKind::Health }
                            span { class: "text-xs text-muted-foreground tabular-nums", "{h:.1} / 20" }
                        } else {
                            span { class: "text-sm text-muted-foreground", "unbekannt" }
                        }
                    }
                    StatBlock { label: "Hunger",
                        if let Some(f) = d.food {
                            IconBar { value: f as f32, kind: BarKind::Food }
                            span { class: "text-xs text-muted-foreground tabular-nums", "{f} / 20" }
                        } else {
                            span { class: "text-sm text-muted-foreground", "unbekannt" }
                        }
                    }
                    StatBlock { label: "Erfahrung",
                        span { class: "text-lg font-semibold tabular-nums",
                            if let Some(x) = d.xp_level { "Level {x}" } else { "–" }
                        }
                    }
                    StatBlock { label: "Spielmodus",
                        if online {
                            Segmented {
                                items: GAMEMODES.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect::<Vec<_>>(),
                                active: d.gamemode.clone().unwrap_or_default(),
                                onchange: move |m: String| act.call(PlayerAction::Gamemode { mode: m }),
                            }
                        } else {
                            span { class: "text-lg font-semibold", "{d.gamemode.as_deref().map(gamemode_label).unwrap_or_else(|| \"–\".into())}" }
                        }
                    }
                    div { class: "sm:col-span-2",
                        StatBlock { label: "Position",
                            if let Some(pos) = d.position.clone() {
                                div { class: "flex flex-wrap items-center gap-3",
                                    span { class: "font-mono text-sm tabular-nums", "X {pos.x:.0} · Y {pos.y:.0} · Z {pos.z:.0}" }
                                    span { class: "inline-flex items-center gap-1 rounded-md bg-muted px-2 py-0.5 text-xs",
                                        icons::Earth { class: "size-3" }
                                        "{dimension_label(&pos.dimension)}"
                                    }
                                    div { class: "flex flex-wrap gap-2",
                                        Button {
                                            size: ButtonSize::Sm,
                                            variant: ButtonVariant::Outline,
                                            onclick: {
                                                let id = id.clone();
                                                let pos = pos.clone();
                                                move |_| {
                                                    *super::map::MAP_FOCUS.write() = Some((pos.x, pos.z, pos.dimension.clone()));
                                                    nav.push(Route::ServerMap { id: id.clone() });
                                                }
                                            },
                                            icons::Map { class: "size-4" }
                                            "Auf Karte zeigen"
                                        }
                                        button {
                                            class: ICON_BUTTON,
                                            title: "Koordinaten kopieren",
                                            aria_label: "Koordinaten kopieren",
                                            onclick: move |_| {
                                                api::copy(&format!("{:.0} {:.0} {:.0}", pos.x, pos.y, pos.z));
                                                toast("Koordinaten kopiert");
                                            },
                                            icons::Copy { class: "size-4" }
                                        }
                                    }
                                }
                            } else {
                                span { class: "text-sm text-muted-foreground", "unbekannt" }
                            }
                        }
                    }
                }

                div { class: "flex flex-col gap-3 rounded-xl border bg-card p-4 shadow-sm sm:p-6",
                    h2 { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "Aktionen" }
                    if !online {
                        p { class: "text-xs text-muted-foreground", "Teleport, Heilen, Nachricht und Kick gehen nur, wenn der Spieler online ist." }
                    }
                    div { class: "grid grid-cols-2 gap-2",
                        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start", disabled: !online, onclick: move |_| dialog.set(PDialog::Teleport),
                            icons::Navigation { class: "size-4" }
                            "Teleport"
                        }
                        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start", disabled: !online, onclick: move |_| dialog.set(PDialog::Message),
                            icons::MessageSquare { class: "size-4" }
                            "Nachricht"
                        }
                        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start", disabled: !online, onclick: move |_| act.call(PlayerAction::Heal),
                            icons::Heart { class: "size-4" }
                            "Heilen"
                        }
                        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start", disabled: !online, onclick: move |_| act.call(PlayerAction::Feed),
                            icons::Drumstick { class: "size-4" }
                            "Sättigen"
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            class: "w-full justify-start",
                            onclick: move |_| act.call(if d.op { PlayerAction::Deop } else { PlayerAction::Op }),
                            icons::Crown { class: "size-4" }
                            if d.op { "OP entziehen" } else { "Zum OP" }
                        }
                        Button {
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            class: "w-full justify-start",
                            onclick: move |_| act.call(if d.whitelisted { PlayerAction::WhitelistRemove } else { PlayerAction::WhitelistAdd }),
                            icons::ListChecks { class: "size-4" }
                            if d.whitelisted { "Whitelist −" } else { "Whitelist +" }
                        }
                        Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start text-destructive", disabled: !online, onclick: move |_| dialog.set(PDialog::Kick),
                            icons::LogOut { class: "size-4" }
                            "Kicken"
                        }
                        if d.banned {
                            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start", onclick: move |_| act.call(PlayerAction::Pardon),
                                icons::ShieldCheck { class: "size-4" }
                                "Entbannen"
                            }
                        } else {
                            Button { variant: ButtonVariant::Outline, size: ButtonSize::Sm, class: "w-full justify-start text-destructive", onclick: move |_| dialog.set(PDialog::Ban),
                                icons::Ban { class: "size-4" }
                                "Bannen"
                            }
                        }
                    }
                }
            }

            // Inventar + Endertruhe
            section { class: "flex flex-col gap-4",
                div { class: "flex flex-wrap items-center justify-between gap-3",
                    h2 { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "Inventar & Endertruhe" }
                    div { class: "flex flex-wrap items-center gap-2",
                        div { class: "xl:hidden",
                            Segmented {
                                items: vec![("inv".to_string(), "Inventar".to_string()), ("ender".to_string(), "Endertruhe".to_string())],
                                active: tab(),
                                onchange: move |v| tab.set(v),
                            }
                        }
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin text-muted-foreground" }
                        }
                        Button {
                            size: ButtonSize::Sm,
                            disabled: !online,
                            title: if online { "Gegenstand per /give geben" } else { "Nur wenn der Spieler online ist" },
                            onclick: move |_| dialog.set(PDialog::Give),
                            icons::Gift { class: "size-4" }
                            "Item geben"
                        }
                    }
                }
                Notice {
                    if online {
                        "Der Spieler ist online: Änderungen werden per Befehl im Spiel ausgeführt. Ziehen auf leere Plätze geht, Tauschen belegter Plätze nur offline."
                    } else {
                        "Der Spieler ist offline: Änderungen werden direkt in der Spielerdatei gespeichert und gelten beim nächsten Beitritt."
                    }
                }
                div { class: "grid items-start gap-6 xl:grid-cols-2",
                    div { class: if tab() == "inv" { "block" } else { "hidden xl:block" },
                        ContainerCard {
                            title: "Inventar",
                            onclear: move |_| dialog.set(PDialog::ClearAll(Container::Inventory)),
                            InventoryGrid {
                                server_id: id.clone(),
                                detail: d.clone(),
                                drag,
                                busy: busy(),
                                onselect: move |s| dialog.set(PDialog::Slot(s)),
                                onop: move |op| inv_op.call(op),
                            }
                        }
                    }
                    div { class: if tab() == "ender" { "block" } else { "hidden xl:block" },
                        ContainerCard {
                            title: "Endertruhe",
                            onclear: move |_| dialog.set(PDialog::ClearAll(Container::EnderChest)),
                            EnderGrid {
                                server_id: id.clone(),
                                detail: d.clone(),
                                drag,
                                busy: busy(),
                                onselect: move |s| dialog.set(PDialog::Slot(s)),
                                onop: move |op| inv_op.call(op),
                            }
                        }
                    }
                }
                p { class: "text-xs text-muted-foreground",
                    "Tipp: Gegenstände mit der Maus zwischen Plätzen und zwischen Inventar und Endertruhe ziehen. Klick auf einen Platz zum Setzen, Ändern oder Leeren."
                }
            }
        }

        // Dialoge
        match dialog() {
            PDialog::None => rsx! {},
            PDialog::Slot(s) => rsx! {
                SlotDialog {
                    server_id: id.clone(),
                    slot: s,
                    item: find_item(&d, s),
                    online,
                    onclose: move |_| dialog.set(PDialog::None),
                    onop: move |op| {
                        dialog.set(PDialog::None);
                        inv_op.call(op);
                    },
                }
            },
            PDialog::Give => rsx! {
                GiveDialog {
                    server_id: id.clone(),
                    name: d.name.clone(),
                    onclose: move |_| dialog.set(PDialog::None),
                    ongive: move |(item, count): (String, u32)| {
                        dialog.set(PDialog::None);
                        inv_op.call(InventoryOp::Give { id: item, count });
                    },
                }
            },
            PDialog::ClearAll(c) => rsx! {
                ConfirmDialog {
                    title: if c == Container::Inventory { "Inventar leeren?" } else { "Endertruhe leeren?" },
                    message: format!(
                        "Alle Gegenstände {} von {} werden entfernt. Das lässt sich nicht rückgängig machen.",
                        if c == Container::Inventory { "im Inventar (inkl. Rüstung und Zweithand)" } else { "in der Endertruhe" },
                        d.name,
                    ),
                    confirm_label: "Alles leeren",
                    destructive: true,
                    oncancel: move |_| dialog.set(PDialog::None),
                    onconfirm: move |_| {
                        dialog.set(PDialog::None);
                        inv_op.call(InventoryOp::ClearAll { container: c });
                    },
                }
            },
            PDialog::Teleport => rsx! {
                TeleportDialog {
                    name: d.name.clone(),
                    position: d.position.clone().map(|p| (p.x, p.y, p.z, p.dimension)),
                    others: other_online.clone(),
                    onclose: move |_| dialog.set(PDialog::None),
                    onsubmit: move |a| {
                        dialog.set(PDialog::None);
                        act.call(a);
                    },
                }
            },
            PDialog::Message => rsx! {
                MessageDialog {
                    name: d.name.clone(),
                    onclose: move |_| dialog.set(PDialog::None),
                    onsubmit: move |text| {
                        dialog.set(PDialog::None);
                        act.call(PlayerAction::Message { text });
                    },
                }
            },
            PDialog::Kick => rsx! {
                ReasonDialog {
                    title: "{d.name} kicken",
                    description: "Der Spieler wird vom Server getrennt und kann sofort wieder beitreten.",
                    confirm_label: "Kicken",
                    onclose: move |_| dialog.set(PDialog::None),
                    onsubmit: move |reason| {
                        dialog.set(PDialog::None);
                        act.call(PlayerAction::Kick { reason });
                    },
                }
            },
            PDialog::Ban => rsx! {
                ReasonDialog {
                    title: "{d.name} bannen",
                    description: "Der Spieler wird getrennt und kann nicht mehr beitreten, bis du ihn entbannst.",
                    confirm_label: "Bannen",
                    onclose: move |_| dialog.set(PDialog::None),
                    onsubmit: move |reason| {
                        dialog.set(PDialog::None);
                        act.call(PlayerAction::Ban { reason });
                    },
                }
            },
        }
    }
}

#[component]
fn StatBlock(#[props(into)] label: String, children: Element) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1.5",
            span { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "{label}" }
            div { class: "flex flex-wrap items-center gap-2", {children} }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BarKind {
    Health,
    Food,
}

/// 10 Herzen bzw. Keulen wie im Spiel (halbe Einheiten möglich).
#[component]
fn IconBar(value: f32, kind: BarKind) -> Element {
    let units: Vec<f32> = (0..10).map(|i| (value / 2.0 - i as f32).clamp(0.0, 1.0)).collect();
    let (full, empty) = match kind {
        BarKind::Health => ("size-4 fill-destructive text-destructive", "size-4 text-muted-foreground/30"),
        BarKind::Food => ("size-4 fill-warning text-warning", "size-4 text-muted-foreground/30"),
    };
    let label = match kind {
        BarKind::Health => format!("Leben {value:.1} von 20"),
        BarKind::Food => format!("Hunger {value:.0} von 20"),
    };
    rsx! {
        div { class: "flex items-center gap-0.5", role: "img", aria_label: "{label}", title: "{label}",
            for (i, f) in units.into_iter().enumerate() {
                span { key: "{i}", class: "relative inline-block size-4",
                    span { class: "absolute inset-0",
                        if kind == BarKind::Health { icons::Heart { class: empty } } else { icons::Drumstick { class: empty } }
                    }
                    if f > 0.0 {
                        span { class: "absolute inset-y-0 left-0 overflow-hidden", style: if f < 1.0 { "width: 50%" } else { "width: 100%" },
                            if kind == BarKind::Health { icons::Heart { class: full } } else { icons::Drumstick { class: full } }
                        }
                    }
                }
            }
        }
    }
}

/* ------------------------------ Inventar ------------------------------ */

#[component]
fn ContainerCard(#[props(into)] title: String, onclear: EventHandler<()>, children: Element) -> Element {
    rsx! {
        div { class: "rounded-xl border bg-card p-4 shadow-sm sm:p-5",
            div { class: "mb-4 flex items-center justify-between gap-2",
                h3 { class: "text-sm font-semibold", "{title}" }
                Dropdown { label: "{title}: Aktionen",
                    button { class: MENU_ITEM_DANGER, onclick: move |_| onclear.call(()),
                        icons::Eraser {}
                        "Alles leeren …"
                    }
                }
            }
            div { class: "flex justify-center", {children} }
        }
    }
}

#[component]
fn InventoryGrid(
    server_id: String,
    detail: PlayerDetail,
    drag: Signal<Option<SlotRef>>,
    busy: bool,
    onselect: EventHandler<SlotRef>,
    onop: EventHandler<InventoryOp>,
) -> Element {
    let c = Container::Inventory;
    let armor: [(i32, &str); 4] = [(103, "Helm"), (102, "Brustpanzer"), (101, "Hose"), (100, "Stiefel")];
    rsx! {
        div { class: "flex flex-col gap-3",
            // Rüstung + Zweithand + Kopf
            div { class: "flex items-end gap-3",
                div { class: "flex flex-col gap-1",
                    for (s, _label) in armor {
                        Slot {
                            key: "a{s}",
                            server_id: server_id.clone(),
                            at: (c, s),
                            item: find_item(&detail, (c, s)),
                            online: detail.online,
                            drag,
                            busy,
                            placeholder: rsx! {
                                match s {
                                    103 => rsx! { icons::HardHat { class: "size-4" } },
                                    102 => rsx! { icons::Shirt { class: "size-4" } },
                                    101 => rsx! { icons::RectangleVertical { class: "size-4" } },
                                    _ => rsx! { icons::Footprints { class: "size-4" } },
                                }
                            },
                            onselect,
                            onop,
                        }
                    }
                }
                div { class: "flex flex-1 flex-col items-center justify-end gap-2 self-stretch rounded-md bg-muted/40 p-3",
                    PlayerHead { player: detail.name.clone(), size: 12 }
                    span { class: "text-xs text-muted-foreground", "{detail.name}" }
                }
                div { class: "flex flex-col items-center gap-1",
                    span { class: "text-[10px] text-muted-foreground", "Zweithand" }
                    Slot {
                        server_id: server_id.clone(),
                        at: (c, -106),
                        item: find_item(&detail, (c, -106)),
                        online: detail.online,
                        drag,
                        busy,
                        placeholder: rsx! { icons::Shield { class: "size-4" } },
                        onselect,
                        onop,
                    }
                }
            }
            // Inventar 9–35
            div { class: "grid grid-cols-9 gap-1",
                for s in 9..=35 {
                    Slot {
                        key: "i{s}",
                        server_id: server_id.clone(),
                        at: (c, s),
                        item: find_item(&detail, (c, s)),
                        online: detail.online,
                        drag,
                        busy,
                        onselect,
                        onop,
                    }
                }
            }
            // Schnellleiste 0–8
            div { class: "mt-1 grid grid-cols-9 gap-1",
                for s in 0..=8 {
                    Slot {
                        key: "h{s}",
                        server_id: server_id.clone(),
                        at: (c, s),
                        item: find_item(&detail, (c, s)),
                        online: detail.online,
                        drag,
                        busy,
                        hotbar: true,
                        onselect,
                        onop,
                    }
                }
            }
        }
    }
}

#[component]
fn EnderGrid(
    server_id: String,
    detail: PlayerDetail,
    drag: Signal<Option<SlotRef>>,
    busy: bool,
    onselect: EventHandler<SlotRef>,
    onop: EventHandler<InventoryOp>,
) -> Element {
    let c = Container::EnderChest;
    let used = detail.ender_chest.iter().filter(|i| !i.id.is_empty() && i.count > 0).count();
    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "grid grid-cols-9 gap-1",
                for s in 0..=26 {
                    Slot {
                        key: "e{s}",
                        server_id: server_id.clone(),
                        at: (c, s),
                        item: find_item(&detail, (c, s)),
                        online: detail.online,
                        drag,
                        busy,
                        onselect,
                        onop,
                    }
                }
            }
            p { class: "text-center text-xs text-muted-foreground", "{used} von 27 Plätzen belegt" }
        }
    }
}

/// Ein Inventarplatz wie im Spiel: Rahmen, Item-Bild, Anzahl, Haltbarkeit, Tooltip, Drag & Drop.
#[component]
fn Slot(
    server_id: String,
    at: SlotRef,
    item: Option<ItemStack>,
    online: bool,
    drag: Signal<Option<SlotRef>>,
    busy: bool,
    #[props(default)] hotbar: bool,
    #[props(optional)] placeholder: Option<Element>,
    onselect: EventHandler<SlotRef>,
    onop: EventHandler<InventoryOp>,
) -> Element {
    let mut drag = drag;
    let mut over = use_signal(|| false);
    let dragging_self = drag() == Some(at);
    let label = slot_label(at.0, at.1);
    let occupied = item.is_some();
    let base = "group relative flex size-8 items-center justify-center rounded-[3px] border-2 border-t-background border-l-background border-r-border border-b-border bg-muted transition-colors sm:size-11";
    let state = if over() {
        "bg-accent ring-2 ring-ring"
    } else if dragging_self {
        "opacity-40"
    } else {
        "hover:bg-accent"
    };
    let hb = if hotbar { "bg-muted/80" } else { "" };
    let durability = item.as_ref().and_then(|i| match (i.damage, i.max_damage) {
        (Some(dmg), Some(max)) if max > 0 && dmg > 0 => Some(((max.saturating_sub(dmg)) as f32 / max as f32).clamp(0.0, 1.0)),
        _ => None,
    });
    let it = item.clone();

    rsx! {
        button {
            r#type: "button",
            class: "{base} {state} {hb}",
            aria_label: if let Some(i) = &item { format!("{label}: {} ×{}", i.custom_name.clone().unwrap_or_else(|| item_display_name(&i.id)), i.count) } else { format!("{label}: leer") },
            draggable: if occupied && !busy { "true" } else { "false" },
            onclick: move |_| onselect.call(at),
            ondragstart: move |e| {
                if !occupied || busy {
                    e.prevent_default();
                    return;
                }
                let dt = e.data_transfer();
                let _ = dt.set_data("text/plain", &label_for_drag(at));
                dt.set_effect_allowed("move");
                drag.set(Some(at));
            },
            ondragend: move |_| {
                drag.set(None);
                over.set(false);
            },
            ondragenter: move |e| {
                if drag.peek().is_some_and(|s| s != at) {
                    e.prevent_default();
                    over.set(true);
                }
            },
            ondragover: move |e| {
                if drag.peek().is_some_and(|s| s != at) {
                    e.prevent_default();
                    e.data_transfer().set_drop_effect("move");
                }
            },
            ondragleave: move |_| over.set(false),
            ondrop: move |e| {
                e.prevent_default();
                over.set(false);
                let from = *drag.peek();
                drag.set(None);
                if let Some(from) = from {
                    if from != at {
                        if online && occupied {
                            toast_err("Tauschen belegter Plätze geht nur, wenn der Spieler offline ist.");
                            return;
                        }
                        onop.call(InventoryOp::Move { from: from.0, from_slot: from.1, to: at.0, to_slot: at.1 });
                    }
                }
            },
            if let Some(i) = it {
                SlotIcon { key: "{i.id}", server_id: server_id.clone(), item: i.id.clone() }
                if !i.enchantments.is_empty() {
                    span { class: "pointer-events-none absolute inset-0 rounded-[1px] bg-info/10" }
                }
                if i.count > 1 {
                    span { class: "pointer-events-none absolute right-0.5 bottom-0 text-[10px] font-bold leading-none tabular-nums text-foreground [text-shadow:1px_1px_0_var(--color-background)] sm:text-xs",
                        "{i.count}"
                    }
                }
                if let Some(d) = durability {
                    span { class: "pointer-events-none absolute right-1 bottom-0.5 left-1 h-[3px] bg-background",
                        span {
                            class: if d > 0.6 { "block h-full bg-success" } else if d > 0.25 { "block h-full bg-warning" } else { "block h-full bg-destructive" },
                            style: "width: {d * 100.0}%",
                        }
                    }
                }
                if drag().is_none() {
                    ItemTooltip { item: i.clone(), label: label.clone() }
                }
            } else if let Some(p) = placeholder {
                span { class: "pointer-events-none text-muted-foreground/40", {p} }
            }
        }
    }
}

fn label_for_drag(at: SlotRef) -> String {
    slot_label(at.0, at.1)
}

/// Item-Bild, das mit dem Platz mitwächst (Ersatz: Anfangsbuchstaben).
#[component]
fn SlotIcon(server_id: String, item: String) -> Element {
    let mut failed = use_signal(|| false);
    let base = item.rsplit(':').next().unwrap_or(&item).to_string();
    let initials: String = base.split('_').filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
    rsx! {
        if failed() {
            span { class: "pointer-events-none flex size-6 items-center justify-center rounded-sm bg-background/60 text-[9px] font-semibold text-muted-foreground sm:size-8", "{initials}" }
        } else {
            img {
                class: "pixelated pointer-events-none size-6 sm:size-8",
                src: "/api/servers/{server_id}/mc/texture/{base}",
                alt: "",
                draggable: "false",
                loading: "lazy",
                onerror: move |_| failed.set(true),
            }
        }
    }
}

#[component]
fn ItemTooltip(item: ItemStack, label: String) -> Element {
    let i = item;
    let name = i.custom_name.clone().unwrap_or_else(|| item_display_name(&i.id));
    let dur = match (i.damage, i.max_damage) {
        (dmg, Some(max)) if max > 0 => Some((max.saturating_sub(dmg.unwrap_or(0)), max)),
        _ => None,
    };
    rsx! {
        span { class: "pointer-events-none invisible absolute bottom-full left-1/2 z-30 mb-2 flex w-max max-w-60 -translate-x-1/2 flex-col gap-0.5 rounded-md border bg-popover px-2.5 py-2 text-left text-xs text-popover-foreground opacity-0 shadow-lg transition-opacity group-hover:visible group-hover:opacity-100 group-focus-visible:visible group-focus-visible:opacity-100",
            span { class: if i.custom_name.is_some() { "font-semibold italic" } else { "font-semibold" }, "{name}" }
            if i.custom_name.is_some() {
                span { class: "text-muted-foreground", "{item_display_name(&i.id)}" }
            }
            for (e, lvl) in i.enchantments.iter().cloned() {
                span { key: "{e}", class: "text-info", "{item_display_name(&e)} {roman(lvl)}" }
            }
            if let Some((left, max)) = dur {
                span { class: "mt-1 flex items-center gap-2 text-muted-foreground",
                    span { class: "h-1 w-16 overflow-hidden rounded-full bg-muted",
                        span {
                            class: if left * 10 > max * 6 { "block h-full bg-success" } else if left * 4 > max { "block h-full bg-warning" } else { "block h-full bg-destructive" },
                            style: "width: {left as f32 / max as f32 * 100.0}%",
                        }
                    }
                    "{left} / {max}"
                }
            }
            span { class: "mt-1 font-mono text-[10px] text-muted-foreground", "{i.id} · ×{i.count}" }
            span { class: "text-[10px] text-muted-foreground", "{label}" }
        }
    }
}

/* ------------------------------ Dialoge ------------------------------ */

/// Item-Auswahl mit Suche im Katalog und Vorschaubildern.
#[component]
fn ItemPicker(server_id: String, selected: Signal<Option<String>>) -> Element {
    let mut selected = selected;
    let mut q = use_signal(String::new);
    let catalog = use_catalog(server_id.clone());
    let query = q().trim().to_lowercase().replace(' ', "_");
    let results: Vec<String> = match &catalog {
        Some(items) => {
            let mut v: Vec<&String> = items.iter().filter(|i| query.is_empty() || i.to_lowercase().contains(&query)).collect();
            if !query.is_empty() {
                v.sort_by_key(|i| {
                    let b = i.rsplit(':').next().unwrap_or(i);
                    (b != query, !b.starts_with(&query), b.len())
                });
            }
            v.into_iter().take(90).cloned().collect()
        }
        None => Vec::new(),
    };
    let first = results.first().cloned();
    rsx! {
        div { class: "flex flex-col gap-2",
            div { class: "relative",
                icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                input {
                    class: "{INPUT} pl-8",
                    placeholder: "Item suchen, z. B. diamond_sword",
                    value: "{q}",
                    autocomplete: "off",
                    spellcheck: "false",
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    oninput: move |e| q.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            e.prevent_default();
                            if let Some(f) = first.clone() {
                                selected.set(Some(f));
                            } else if q().contains(':') {
                                selected.set(Some(q().trim().to_string()));
                            }
                        }
                    },
                }
            }
            if catalog.is_none() {
                Loading { text: "Lade Item-Katalog …" }
            } else if results.is_empty() {
                p { class: "py-6 text-center text-sm text-muted-foreground",
                    "Kein Item gefunden. Eigene IDs (z. B. aus Mods) mit Namensraum eingeben und Enter drücken."
                }
            } else {
                div { class: "grid max-h-64 grid-cols-[repeat(auto-fill,minmax(4.5rem,1fr))] gap-1 overflow-y-auto rounded-md border bg-muted/30 p-1",
                    for it in results {
                        {
                            let sel = selected.read().as_deref() == Some(it.as_str());
                            rsx! {
                                button {
                                    key: "{it}",
                                    r#type: "button",
                                    class: if sel { "flex flex-col items-center gap-1 rounded-md bg-accent p-1.5 ring-2 ring-ring" } else { "flex flex-col items-center gap-1 rounded-md p-1.5 hover:bg-accent" },
                                    title: "{it}",
                                    onclick: {
                                        let it = it.clone();
                                        move |_| selected.set(Some(it.clone()))
                                    },
                                    SlotIcon { server_id: server_id.clone(), item: it.clone() }
                                    span { class: "line-clamp-2 w-full text-center text-[10px] leading-tight text-muted-foreground", "{item_display_name(&it)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SlotDialog(server_id: String, slot: SlotRef, item: Option<ItemStack>, online: bool, onclose: EventHandler<()>, onop: EventHandler<InventoryOp>) -> Element {
    let (c, s) = slot;
    let mut count = use_signal(|| item.as_ref().map_or(1, |i| i.count));
    let mut new_count = use_signal(|| 1u32);
    let picked = use_signal(|| None::<String>);
    let mut replace = use_signal(|| item.is_none());
    let label = slot_label(c, s);
    let i2 = item.clone();

    rsx! {
        Modal {
            title: "{label}",
            description: if item.is_some() { "Gegenstand ansehen, Anzahl ändern, ersetzen oder entfernen." } else { "Leerer Platz – wähle einen Gegenstand." },
            wide: replace(),
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Schliessen" }
                if replace() {
                    Button {
                        disabled: picked().is_none(),
                        onclick: move |_| {
                            if let Some(id) = picked() {
                                onop.call(InventoryOp::Set { container: c, slot: s, id, count: new_count().clamp(1, 99) });
                            }
                        },
                        icons::Check { class: "size-4" }
                        if i2.is_some() { "Ersetzen" } else { "Setzen" }
                    }
                }
            },
            div { class: "flex flex-col gap-5",
                if let Some(i) = item.clone() {
                    div { class: "flex items-start gap-4 rounded-lg border bg-muted/30 p-3",
                        div { class: "flex size-14 shrink-0 items-center justify-center rounded-[3px] border-2 border-t-background border-l-background border-r-border border-b-border bg-muted",
                            SlotIcon { key: "{i.id}", server_id: server_id.clone(), item: i.id.clone() }
                        }
                        div { class: "min-w-0 flex-1 text-sm",
                            p { class: "font-medium", "{i.custom_name.clone().unwrap_or_else(|| item_display_name(&i.id))}" }
                            if i.custom_name.is_some() {
                                p { class: "text-xs text-muted-foreground", "Eigener Name · {item_display_name(&i.id)}" }
                            }
                            p { class: "font-mono text-xs text-muted-foreground", "{i.id}" }
                            if !i.enchantments.is_empty() {
                                div { class: "mt-1.5 flex flex-wrap gap-1",
                                    for (e, l) in i.enchantments.iter().cloned() {
                                        span { key: "{e}", class: "rounded bg-info/15 px-1.5 py-0.5 text-[11px] text-info", "{item_display_name(&e)} {roman(l)}" }
                                    }
                                }
                            }
                            if let (Some(max), dmg) = (i.max_damage, i.damage) {
                                if max > 0 {
                                    {
                                        let left = max.saturating_sub(dmg.unwrap_or(0));
                                        let pct = left as f32 / max as f32 * 100.0;
                                        rsx! {
                                            div { class: "mt-2 flex items-center gap-2 text-xs text-muted-foreground",
                                                span { "Haltbarkeit" }
                                                span { class: "h-1.5 w-24 overflow-hidden rounded-full bg-muted",
                                                    span {
                                                        class: if pct > 60.0 { "block h-full bg-success" } else if pct > 25.0 { "block h-full bg-warning" } else { "block h-full bg-destructive" },
                                                        style: "width: {pct}%",
                                                    }
                                                }
                                                span { class: "tabular-nums", "{left} / {max}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Anzahl
                    form {
                        class: "flex flex-wrap items-end gap-2",
                        onsubmit: move |e| {
                            e.prevent_default();
                            onop.call(InventoryOp::SetCount { container: c, slot: s, count: count().clamp(1, 99) });
                        },
                        Field { label: "Anzahl", class: "w-28",
                            input {
                                class: INPUT,
                                r#type: "number",
                                min: "1",
                                max: "99",
                                value: "{count}",
                                oninput: move |e| count.set(e.value().parse().unwrap_or(1)),
                            }
                        }
                        Button { variant: ButtonVariant::Outline, button_type: "submit", disabled: count() == i.count || count() == 0, "Anzahl übernehmen" }
                        div { class: "flex-1" }
                        Button { variant: ButtonVariant::Destructive, onclick: move |_| onop.call(InventoryOp::Clear { container: c, slot: s }),
                            icons::Trash2 { class: "size-4" }
                            "Leeren"
                        }
                    }
                    if !replace() {
                        Button { variant: ButtonVariant::Ghost, size: ButtonSize::Sm, class: "self-start", onclick: move |_| replace.set(true),
                            icons::ArrowLeftRight { class: "size-4" }
                            "Durch anderen Gegenstand ersetzen …"
                        }
                    }
                    if online && i.has_extra_data && replace() {
                        Notice { warning: true, "Dieser Gegenstand hat zusätzliche Daten (Komponenten/Tags), die beim Ersetzen verloren gehen." }
                    }
                }
                if replace() {
                    div { class: "flex flex-col gap-3",
                        ItemPicker { server_id: server_id.clone(), selected: picked }
                        div { class: "flex flex-wrap items-end gap-3",
                            Field { label: "Anzahl", class: "w-28",
                                input {
                                    class: INPUT,
                                    r#type: "number",
                                    min: "1",
                                    max: "99",
                                    value: "{new_count}",
                                    oninput: move |e| new_count.set(e.value().parse().unwrap_or(1)),
                                }
                            }
                            if let Some(p) = picked() {
                                span { class: "pb-2 text-sm text-muted-foreground", "Gewählt: " span { class: "font-medium text-foreground", "{item_display_name(&p)}" } }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn GiveDialog(server_id: String, name: String, onclose: EventHandler<()>, ongive: EventHandler<(String, u32)>) -> Element {
    let picked = use_signal(|| None::<String>);
    let mut count = use_signal(|| 1u32);
    rsx! {
        Modal {
            title: "Item geben",
            description: "{name} erhält den Gegenstand per /give (landet im Inventar oder fällt zu Boden).",
            wide: true,
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button {
                    disabled: picked().is_none(),
                    onclick: move |_| {
                        if let Some(p) = picked() {
                            ongive.call((p, count().clamp(1, 6400)));
                        }
                    },
                    icons::Gift { class: "size-4" }
                    "Geben"
                }
            },
            div { class: "flex flex-col gap-3",
                ItemPicker { server_id: server_id.clone(), selected: picked }
                div { class: "flex flex-wrap items-end gap-3",
                    Field { label: "Anzahl", class: "w-28",
                        input {
                            class: INPUT,
                            r#type: "number",
                            min: "1",
                            max: "6400",
                            value: "{count}",
                            oninput: move |e| count.set(e.value().parse().unwrap_or(1)),
                        }
                    }
                    for n in [1u32, 16, 32, 64] {
                        Button { key: "{n}", variant: ButtonVariant::Outline, size: ButtonSize::Sm, onclick: move |_| count.set(n), "{n}" }
                    }
                }
            }
        }
    }
}

#[component]
fn TeleportDialog(
    name: String,
    position: Option<(f64, f64, f64, String)>,
    others: Vec<String>,
    onclose: EventHandler<()>,
    onsubmit: EventHandler<PlayerAction>,
) -> Element {
    let p = position.clone().unwrap_or((0.0, 64.0, 0.0, "minecraft:overworld".into()));
    let mut mode = use_signal(|| "coords".to_string());
    let mut x = use_signal(|| format!("{:.0}", p.0));
    let mut y = use_signal(|| format!("{:.0}", p.1));
    let mut z = use_signal(|| format!("{:.0}", p.2));
    let mut dim = use_signal(|| p.3.clone());
    let mut target = use_signal(|| others.first().cloned().unwrap_or_default());
    let mut dims = vec!["minecraft:overworld".to_string(), "minecraft:the_nether".to_string(), "minecraft:the_end".to_string()];
    if !dims.contains(&p.3) {
        dims.push(p.3.clone());
    }
    let parsed = (x().trim().parse::<f64>(), y().trim().parse::<f64>(), z().trim().parse::<f64>());
    let coords_ok = parsed.0.is_ok() && parsed.1.is_ok() && parsed.2.is_ok();
    let can = if mode() == "coords" { coords_ok } else { !target().is_empty() };
    let submit = move || {
        if mode() == "coords" {
            if let (Ok(x), Ok(y), Ok(z)) = (x().trim().parse::<f64>(), y().trim().parse::<f64>(), z().trim().parse::<f64>()) {
                onsubmit.call(PlayerAction::Teleport { x, y, z, dimension: Some(dim()) });
            }
        } else if !target().is_empty() {
            onsubmit.call(PlayerAction::TeleportTo { target: target() });
        }
    };
    rsx! {
        Modal {
            title: "{name} teleportieren",
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button { disabled: !can, onclick: move |_| submit(),
                    icons::Navigation { class: "size-4" }
                    "Teleportieren"
                }
            },
            form {
                class: "flex flex-col gap-4",
                onsubmit: move |e| {
                    e.prevent_default();
                    if can {
                        submit();
                    }
                },
                Segmented {
                    items: vec![("coords".to_string(), "Zu Koordinaten".to_string()), ("player".to_string(), "Zu Spieler".to_string())],
                    active: mode(),
                    onchange: move |v| mode.set(v),
                }
                if mode() == "coords" {
                    div { class: "grid grid-cols-3 gap-2",
                        Field { label: "X",
                            input { class: INPUT, inputmode: "decimal", value: "{x}", oninput: move |e| x.set(e.value()) }
                        }
                        Field { label: "Y",
                            input { class: INPUT, inputmode: "decimal", value: "{y}", oninput: move |e| y.set(e.value()) }
                        }
                        Field { label: "Z",
                            input { class: INPUT, inputmode: "decimal", value: "{z}", oninput: move |e| z.set(e.value()) }
                        }
                    }
                    Field { label: "Dimension",
                        select { class: SELECT, value: "{dim}", onchange: move |e| dim.set(e.value()),
                            for d in dims {
                                option { key: "{d}", value: "{d}", selected: d == dim(), "{dimension_label(&d)}" }
                            }
                        }
                    }
                    if !coords_ok {
                        p { class: "text-xs text-destructive", "Bitte gültige Zahlen eingeben." }
                    }
                } else if others.is_empty() {
                    p { class: "py-4 text-center text-sm text-muted-foreground", "Kein anderer Spieler ist online." }
                } else {
                    div { class: "flex flex-col gap-1",
                        for o in others.iter().cloned() {
                            button {
                                key: "{o}",
                                r#type: "button",
                                class: if target() == o { "flex items-center gap-2 rounded-md bg-accent px-2 py-1.5 text-sm ring-2 ring-ring" } else { "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent" },
                                onclick: {
                                    let o = o.clone();
                                    move |_| target.set(o.clone())
                                },
                                PlayerHead { player: o.clone(), size: 6 }
                                "{o}"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MessageDialog(name: String, onclose: EventHandler<()>, onsubmit: EventHandler<String>) -> Element {
    let mut text = use_signal(String::new);
    let submit = move || {
        let t = text().trim().to_string();
        if !t.is_empty() {
            onsubmit.call(t);
        }
    };
    rsx! {
        Modal {
            title: "Nachricht an {name}",
            description: "Erscheint nur bei diesem Spieler im Chat.",
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button { disabled: text().trim().is_empty(), onclick: move |_| submit(),
                    icons::Send { class: "size-4" }
                    "Senden"
                }
            },
            form {
                onsubmit: move |e| {
                    e.prevent_default();
                    submit();
                },
                input {
                    class: INPUT,
                    placeholder: "Nachricht",
                    value: "{text}",
                    maxlength: "256",
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    oninput: move |e| text.set(e.value()),
                }
            }
        }
    }
}
