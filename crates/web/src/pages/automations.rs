//! Automationen: Ereignis-Verlauf, Liste der Automationen, Editor mit Vorlagen.

use dioxus::prelude::*;
use nrz_gp_common::{Action, Automation, EventKind, Game, GameEvent, SaveAutomation, Trigger, routes};

use crate::{
    api,
    components::{
        CARD, CheckRow, ConfirmDialog, ErrorBox, Field, INPUT, Loading, Modal, SELECT, Segmented, Tag, Toggle, fmt_ago,
        fmt_datetime, fmt_datetime_ms,
        ui::{
            app_shell::EmptyState,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::use_server,
    pages::dashboard::event_icon,
    store::{toast, toast_err},
};

/* ------------------------------------------------------------------ */
/*                           Lesbare Texte                            */
/* ------------------------------------------------------------------ */

fn trigger_text(t: &Trigger) -> String {
    match t {
        Trigger::Event { event, filter } => match filter.as_deref().filter(|f| !f.is_empty()) {
            Some(f) => format!("Bei „{}“ mit „{f}“", event.label()),
            None => format!("Bei „{}“", event.label()),
        },
        Trigger::Interval { minutes } if *minutes >= 60 && minutes % 60 == 0 => format!("Alle {} Std", minutes / 60),
        Trigger::Interval { minutes } => format!("Alle {minutes} Min"),
        Trigger::Daily { hour, minute } => format!("Täglich um {hour:02}:{minute:02}"),
    }
}

fn action_label(a: &Action) -> &'static str {
    match a {
        Action::Command { .. } => "Befehl",
        Action::Broadcast { .. } => "Nachricht an alle",
        Action::Start => "Starten",
        Action::Stop => "Stoppen",
        Action::Restart => "Neustart",
        Action::Backup { .. } => "Backup",
        Action::Webhook { .. } => "Webhook",
        Action::Delay { .. } => "Warten",
    }
}

fn action_summary(a: &Action) -> String {
    match a {
        Action::Command { command } => format!("/{}", command.trim_start_matches('/')),
        Action::Broadcast { message } => format!("„{message}“"),
        Action::Backup { worlds_only: true } => "Backup (Welten)".into(),
        Action::Backup { worlds_only: false } => "Backup (komplett)".into(),
        Action::Webhook { discord: true, .. } => "Discord".into(),
        Action::Webhook { .. } => "Webhook".into(),
        Action::Delay { seconds } => format!("{seconds} s warten"),
        other => action_label(other).into(),
    }
}

fn action_icon(a: &Action) -> Element {
    match a {
        Action::Command { .. } => rsx! { icons::SquareTerminal { class: "size-4" } },
        Action::Broadcast { .. } => rsx! { icons::MessageSquare { class: "size-4" } },
        Action::Start => rsx! { icons::Play { class: "size-4" } },
        Action::Stop => rsx! { icons::Square { class: "size-4" } },
        Action::Restart => rsx! { icons::RotateCw { class: "size-4" } },
        Action::Backup { .. } => rsx! { icons::Archive { class: "size-4" } },
        Action::Webhook { .. } => rsx! { icons::Webhook { class: "size-4" } },
        Action::Delay { .. } => rsx! { icons::Hourglass { class: "size-4" } },
    }
}

fn trigger_icon(t: &Trigger) -> Element {
    match t {
        Trigger::Event { .. } => rsx! { icons::Zap { class: "size-4" } },
        Trigger::Interval { .. } => rsx! { icons::Timer { class: "size-4" } },
        Trigger::Daily { .. } => rsx! { icons::CalendarClock { class: "size-4" } },
    }
}

fn event_kind_key(k: EventKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn event_kind_from_key(s: &str) -> Option<EventKind> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

/* ------------------------------------------------------------------ */
/*                              Vorlagen                              */
/* ------------------------------------------------------------------ */

fn templates(game: Game) -> Vec<(&'static str, SaveAutomation)> {
    let welcome = if game == Game::Minecraft { "Willkommen auf dem Server, {player}!" } else { "Willkommen, {player}!" };
    vec![
        (
            "Täglicher Neustart 04:00",
            SaveAutomation {
                name: "Täglicher Neustart".into(),
                enabled: true,
                trigger: Trigger::Daily { hour: 4, minute: 0 },
                actions: vec![
                    Action::Broadcast { message: "Der Server startet in 60 Sekunden neu.".into() },
                    Action::Delay { seconds: 60 },
                    Action::Restart,
                ],
            },
        ),
        (
            "Backup alle 6 Std",
            SaveAutomation {
                name: "Backup alle 6 Stunden".into(),
                enabled: true,
                trigger: Trigger::Interval { minutes: 360 },
                actions: vec![Action::Backup { worlds_only: true }],
            },
        ),
        (
            "Willkommensnachricht",
            SaveAutomation {
                name: "Willkommensnachricht".into(),
                enabled: true,
                trigger: Trigger::Event { event: EventKind::PlayerJoin, filter: None },
                actions: vec![Action::Broadcast { message: welcome.into() }],
            },
        ),
        (
            "Discord bei Absturz",
            SaveAutomation {
                name: "Discord bei Absturz".into(),
                enabled: true,
                trigger: Trigger::Event { event: EventKind::ServerCrashed, filter: None },
                actions: vec![Action::Webhook { url: String::new(), discord: true }],
            },
        ),
    ]
}

#[derive(Clone, PartialEq)]
struct Draft {
    /// None = neu.
    id: Option<String>,
    save: SaveAutomation,
}

/* ------------------------------------------------------------------ */
/*                                Seite                               */
/* ------------------------------------------------------------------ */

#[component]
pub fn ServerAutomations(id: String) -> Element {
    let ctx = use_server();
    let game = ctx.game();
    let mut reload = use_signal(|| 0u32);
    let list = use_resource(move || {
        let id = ctx.id();
        let _ = reload();
        async move { api::get::<Vec<Automation>>(&routes::server(&id, "automations")).await }
    });
    let mut editing = use_signal(|| None::<Draft>);
    let mut deleting = use_signal(|| None::<Automation>);
    let mut running = use_signal(|| None::<String>);

    let save_enabled = move |a: Automation, enabled: bool| {
        let id = ctx.id();
        spawn(async move {
            let body = SaveAutomation { name: a.name.clone(), enabled, trigger: a.trigger.clone(), actions: a.actions.clone() };
            match api::put::<_, Automation>(&routes::server(&id, &format!("automations/{}", a.id)), &body).await {
                Ok(_) => {
                    toast(if enabled { "Automation aktiviert" } else { "Automation pausiert" });
                    reload += 1;
                }
                Err(e) => toast_err(e),
            }
        });
    };

    let confirm_delete = move |_| {
        let Some(a) = deleting() else { return };
        deleting.set(None);
        let id = ctx.id();
        spawn(async move {
            match api::delete::<nrz_gp_common::Ok>(&routes::server(&id, &format!("automations/{}", a.id))).await {
                Ok(_) => {
                    toast("Automation gelöscht");
                    reload += 1;
                }
                Err(e) => toast_err(e),
            }
        });
    };

    let new_blank = move |_| {
        editing.set(Some(Draft {
            id: None,
            save: SaveAutomation { name: String::new(), enabled: true, trigger: Trigger::default(), actions: Vec::new() },
        }))
    };

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-8 p-4 md:p-8",
            EventHistory {}

            section { class: "flex flex-col gap-4",
                div { class: "flex flex-wrap items-end justify-between gap-3",
                    div {
                        h2 { class: "text-lg font-semibold tracking-tight", "Automationen" }
                        p { class: "text-sm text-muted-foreground", "Wenn etwas passiert oder zu einer bestimmten Zeit – dann führt das Panel Aktionen aus." }
                    }
                    Button { onclick: new_blank,
                        icons::Plus { class: "size-4" }
                        "Neue Automation"
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    span { class: "text-xs text-muted-foreground", "Vorlagen:" }
                    for (label, tpl) in templates(game) {
                        Button {
                            key: "{label}",
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            onclick: move |_| editing.set(Some(Draft { id: None, save: tpl.clone() })),
                            icons::Sparkles { class: "size-3.5" }
                            "{label}"
                        }
                    }
                }

                match &*list.read() {
                    None => rsx! { Loading { text: "Lade Automationen …" } },
                    Some(Err(e)) => rsx! { ErrorBox { message: e.clone() } },
                    Some(Ok(items)) if items.is_empty() => rsx! {
                        EmptyState {
                            title: "Noch keine Automationen",
                            description: "Starte mit einer Vorlage oben, z. B. einem täglichen Neustart mit Warnung oder regelmässigen Backups.",
                            icon: rsx! { icons::Zap {} },
                        }
                    },
                    Some(Ok(items)) => rsx! {
                        div { class: "{CARD} divide-y",
                            for a in items.iter().cloned() {
                                {
                                    let a_toggle = a.clone();
                                    let a_edit = a.clone();
                                    let a_del = a.clone();
                                    let aid = a.id.clone();
                                    let is_running = running().as_deref() == Some(a.id.as_str());
                                    rsx! {
                                        div { key: "{a.id}", class: "flex flex-wrap items-start gap-3 px-4 py-3",
                                            div { class: if a.enabled { "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted" } else { "mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground" },
                                                {trigger_icon(&a.trigger)}
                                            }
                                            div { class: "min-w-0 flex-1",
                                                div { class: "flex flex-wrap items-center gap-2",
                                                    span { class: if a.enabled { "font-medium" } else { "font-medium text-muted-foreground" }, "{a.name}" }
                                                    if !a.enabled { Tag { "Pausiert" } }
                                                }
                                                p { class: "text-sm text-muted-foreground", "{trigger_text(&a.trigger)}" }
                                                div { class: "mt-1.5 flex flex-wrap items-center gap-1",
                                                    for (i, act) in a.actions.iter().enumerate() {
                                                        if i > 0 {
                                                            icons::ChevronRight { class: "size-3 text-muted-foreground" }
                                                        }
                                                        span { class: "inline-flex max-w-64 items-center gap-1 truncate rounded-md bg-muted px-1.5 py-0.5 text-xs [&_svg]:size-3",
                                                            {action_icon(act)}
                                                            span { class: "truncate", "{action_summary(act)}" }
                                                        }
                                                    }
                                                }
                                                p { class: "mt-1.5 text-xs text-muted-foreground",
                                                    match a.last_run {
                                                        Some(t) => rsx! { span { title: "{fmt_datetime(t)}", "Zuletzt ausgeführt {fmt_ago(t)}" } },
                                                        None => rsx! { "Noch nie ausgeführt" },
                                                    }
                                                }
                                            }
                                            div { class: "flex items-center gap-1",
                                                Toggle {
                                                    checked: a.enabled,
                                                    label: "Aktiv",
                                                    onchange: move |v| save_enabled(a_toggle.clone(), v),
                                                }
                                                Button {
                                                    size: ButtonSize::IconSm,
                                                    variant: ButtonVariant::Ghost,
                                                    title: "Jetzt ausführen",
                                                    aria_label: "Jetzt ausführen",
                                                    disabled: is_running,
                                                    onclick: move |_| {
                                                        let id = ctx.id();
                                                        let aid = aid.clone();
                                                        running.set(Some(aid.clone()));
                                                        spawn(async move {
                                                            match api::post_empty::<nrz_gp_common::Ok>(&routes::server(&id, &format!("automations/{aid}/run"))).await {
                                                                Ok(_) => {
                                                                    toast("Automation wird ausgeführt");
                                                                    reload += 1;
                                                                }
                                                                Err(e) => toast_err(e),
                                                            }
                                                            running.set(None);
                                                        });
                                                    },
                                                    if is_running {
                                                        icons::LoaderCircle { class: "size-4 animate-spin" }
                                                    } else {
                                                        icons::Play { class: "size-4" }
                                                    }
                                                }
                                                Button {
                                                    size: ButtonSize::IconSm,
                                                    variant: ButtonVariant::Ghost,
                                                    title: "Bearbeiten",
                                                    aria_label: "Bearbeiten",
                                                    onclick: move |_| editing.set(Some(Draft {
                                                        id: Some(a_edit.id.clone()),
                                                        save: SaveAutomation {
                                                            name: a_edit.name.clone(),
                                                            enabled: a_edit.enabled,
                                                            trigger: a_edit.trigger.clone(),
                                                            actions: a_edit.actions.clone(),
                                                        },
                                                    })),
                                                    icons::Pencil { class: "size-4" }
                                                }
                                                Button {
                                                    size: ButtonSize::IconSm,
                                                    variant: ButtonVariant::Ghost,
                                                    title: "Löschen",
                                                    aria_label: "Löschen",
                                                    onclick: move |_| deleting.set(Some(a_del.clone())),
                                                    icons::Trash2 { class: "size-4 text-destructive" }
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

        if let Some(d) = editing() {
            AutomationEditor {
                draft: d,
                game,
                onclose: move |_| editing.set(None),
                onsaved: move |_| {
                    editing.set(None);
                    reload += 1;
                },
            }
        }
        if let Some(a) = deleting() {
            ConfirmDialog {
                title: "Automation löschen?",
                message: format!("„{}“ wird endgültig gelöscht.", a.name),
                confirm_label: "Löschen",
                destructive: true,
                onconfirm: confirm_delete,
                oncancel: move |_| deleting.set(None),
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                           Ereignis-Verlauf                         */
/* ------------------------------------------------------------------ */

#[component]
fn EventHistory() -> Element {
    let ctx = use_server();
    let fetched = use_resource(move || {
        let id = ctx.id();
        async move { api::get::<Vec<GameEvent>>(&format!("{}?limit=500", routes::server(&id, "events"))).await }
    });
    let mut kind = use_signal(String::new);
    let mut query = use_signal(String::new);

    let loading = fetched.read().is_none();
    let error = match &*fetched.read() {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    // Geladene + live eingetroffene Ereignisse zusammenführen
    let mut all: Vec<GameEvent> = ctx.events.read().clone();
    if let Some(Ok(list)) = &*fetched.read() {
        for e in list {
            if !all.iter().any(|x| x.id == e.id) {
                all.push(e.clone());
            }
        }
    }
    all.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.id.cmp(&a.id)));
    let filter_kind = event_kind_from_key(&kind());
    let q = query().trim().to_lowercase();
    let shown: Vec<GameEvent> = all
        .into_iter()
        .filter(|e| filter_kind.map_or(e.kind != EventKind::LogError, |k| e.kind == k))
        .filter(|e| q.is_empty() || e.message.to_lowercase().contains(&q) || e.player.as_deref().is_some_and(|p| p.to_lowercase().contains(&q)))
        .take(300)
        .collect();

    rsx! {
        section { class: "flex flex-col gap-3",
            div { class: "flex flex-wrap items-end justify-between gap-3",
                div {
                    h2 { class: "text-lg font-semibold tracking-tight", "Ereignis-Verlauf" }
                    p { class: "text-sm text-muted-foreground", "Was auf dem Server passiert ist – jedes Ereignis kann eine Automation auslösen." }
                }
                div { class: "flex w-full flex-wrap gap-2 sm:w-auto",
                    div { class: "relative min-w-0 flex-1 sm:w-56 sm:flex-none",
                        icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                        input {
                            class: "{INPUT} pl-8",
                            r#type: "search",
                            placeholder: "Spieler oder Text …",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                            onkeydown: move |e| if e.key() == Key::Escape { query.set(String::new()) },
                        }
                    }
                    select { class: "{SELECT} w-auto", value: "{kind}", onchange: move |e| kind.set(e.value()),
                        option { value: "", "Alle Ereignisse" }
                        for k in EventKind::ALL.iter().copied() {
                            option { value: "{event_kind_key(k)}", selected: kind() == event_kind_key(k), "{k.label()}" }
                        }
                    }
                }
            }
            if let Some(e) = error {
                ErrorBox { message: e }
            }
            div { class: "{CARD} max-h-96 divide-y overflow-y-auto",
                if loading && shown.is_empty() {
                    Loading { text: "Lade Ereignisse …" }
                } else if shown.is_empty() {
                    div { class: "px-4 py-8 text-center text-sm text-muted-foreground",
                        if q.is_empty() && filter_kind.is_none() { "Noch keine Ereignisse" } else { "Keine passenden Ereignisse" }
                    }
                }
                for e in shown {
                    div { key: "{e.id}", class: "flex items-start gap-2.5 px-3 py-2",
                        div { class: "mt-0.5", {event_icon(e.kind)} }
                        div { class: "min-w-0 flex-1 text-sm",
                            div { class: "flex flex-wrap items-center gap-x-1.5",
                                if let Some(p) = &e.player {
                                    span { class: "font-medium", "{p}" }
                                }
                                span { class: "min-w-0 break-words text-muted-foreground",
                                    if e.kind == EventKind::Chat { "„{e.message}“" } else { "{e.message}" }
                                }
                            }
                            div { class: "text-[11px] text-muted-foreground", "{fmt_datetime_ms(e.ts)} · {e.kind.label()}" }
                        }
                    }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                               Editor                               */
/* ------------------------------------------------------------------ */

const ACTION_KINDS: &[(&str, &str)] = &[
    ("command", "Konsolenbefehl"),
    ("broadcast", "Nachricht an alle"),
    ("start", "Server starten"),
    ("stop", "Server stoppen"),
    ("restart", "Server neu starten"),
    ("backup", "Backup erstellen"),
    ("webhook", "Webhook / Discord"),
    ("delay", "Warten"),
];

fn new_action(kind: &str) -> Option<Action> {
    Some(match kind {
        "command" => Action::Command { command: String::new() },
        "broadcast" => Action::Broadcast { message: String::new() },
        "start" => Action::Start,
        "stop" => Action::Stop,
        "restart" => Action::Restart,
        "backup" => Action::Backup { worlds_only: true },
        "webhook" => Action::Webhook { url: String::new(), discord: true },
        "delay" => Action::Delay { seconds: 30 },
        _ => return None,
    })
}

fn validate(s: &SaveAutomation) -> Option<String> {
    if s.name.trim().is_empty() {
        return Some("Bitte einen Namen eingeben.".into());
    }
    if let Trigger::Interval { minutes } = s.trigger
        && minutes == 0
    {
        return Some("Das Intervall muss mindestens 1 Minute sein.".into());
    }
    if s.actions.is_empty() {
        return Some("Füge mindestens eine Aktion hinzu.".into());
    }
    for a in &s.actions {
        match a {
            Action::Command { command } if command.trim().is_empty() => return Some("Ein Befehl ist leer.".into()),
            Action::Broadcast { message } if message.trim().is_empty() => return Some("Eine Nachricht ist leer.".into()),
            Action::Webhook { url, .. } if !(url.starts_with("http://") || url.starts_with("https://")) => {
                return Some("Bitte eine gültige Webhook-Adresse (https://…) eingeben.".into());
            }
            _ => {}
        }
    }
    None
}

#[component]
fn AutomationEditor(draft: Draft, game: Game, onclose: EventHandler<()>, onsaved: EventHandler<()>) -> Element {
    let ctx = use_server();
    let is_new = draft.id.is_none();
    let aid_sig = use_signal(|| draft.id.clone());
    let mut s = use_signal(|| draft.save.clone());
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut add_kind = use_signal(String::new);

    let mut submit = move || {
        let body = s();
        if let Some(e) = validate(&body) {
            error.set(Some(e));
            return;
        }
        let mut body = body;
        body.name = body.name.trim().to_string();
        if let Trigger::Event { filter, .. } = &mut body.trigger
            && filter.as_deref().is_some_and(|f| f.trim().is_empty())
        {
            *filter = None;
        }
        error.set(None);
        busy.set(true);
        let id = ctx.id();
        let aid = aid_sig();
        spawn(async move {
            let res = match &aid {
                Some(aid) => api::put::<_, Automation>(&routes::server(&id, &format!("automations/{aid}")), &body).await,
                None => api::post::<_, Automation>(&routes::server(&id, "automations"), &body).await,
            };
            busy.set(false);
            match res {
                Ok(_) => {
                    toast(if aid.is_some() { "Automation gespeichert" } else { "Automation angelegt" });
                    onsaved.call(());
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let cur = s();
    let trig_kind = match cur.trigger {
        Trigger::Event { .. } => "event",
        Trigger::Interval { .. } => "interval",
        Trigger::Daily { .. } => "daily",
    };
    let count = cur.actions.len();
    let is_event = matches!(cur.trigger, Trigger::Event { .. });

    rsx! {
        Modal {
            title: if is_new { "Neue Automation" } else { "Automation bearbeiten" },
            wide: true,
            onclose: move |_| onclose.call(()),
            footer: rsx! {
                Button { variant: ButtonVariant::Outline, onclick: move |_| onclose.call(()), "Abbrechen" }
                Button { disabled: busy(), onclick: move |_| submit(),
                    if busy() {
                        icons::LoaderCircle { class: "size-4 animate-spin" }
                    }
                    if is_new { "Anlegen" } else { "Speichern" }
                }
            },
            form {
                class: "flex flex-col gap-5",
                onsubmit: move |e| {
                    e.prevent_default();
                    submit();
                },
                if let Some(e) = error() {
                    ErrorBox { message: e }
                }
                div { class: "flex flex-wrap items-end gap-4",
                    Field { label: "Name", class: "min-w-0 flex-1",
                        input {
                            class: INPUT,
                            value: "{cur.name}",
                            placeholder: "z. B. Täglicher Neustart",
                            autofocus: true,
                            oninput: move |e| s.write().name = e.value(),
                        }
                    }
                    div { class: "flex h-9 items-center gap-2 text-sm",
                        Toggle { checked: cur.enabled, label: "Aktiv", onchange: move |v| s.write().enabled = v }
                        span { if cur.enabled { "Aktiv" } else { "Pausiert" } }
                    }
                }

                // Auslöser
                div { class: "flex flex-col gap-3",
                    h3 { class: "text-sm font-semibold", "Wann?" }
                    Segmented {
                        items: vec![
                            ("event".to_string(), "Bei Ereignis".to_string()),
                            ("interval".to_string(), "Intervall".to_string()),
                            ("daily".to_string(), "Täglich".to_string()),
                        ],
                        active: trig_kind.to_string(),
                        onchange: move |v: String| {
                            let t = match v.as_str() {
                                "event" => Trigger::Event { event: EventKind::PlayerJoin, filter: None },
                                "interval" => Trigger::Interval { minutes: 60 },
                                _ => Trigger::Daily { hour: 4, minute: 0 },
                            };
                            if v != trig_kind {
                                s.write().trigger = t;
                            }
                        },
                    }
                    match cur.trigger.clone() {
                        Trigger::Event { event, filter } => rsx! {
                            div { class: "grid gap-3 sm:grid-cols-2",
                                Field { label: "Ereignis",
                                    select {
                                        class: SELECT,
                                        onchange: move |e| {
                                            if let Some(k) = event_kind_from_key(&e.value())
                                                && let Trigger::Event { event, .. } = &mut s.write().trigger
                                            {
                                                *event = k;
                                            }
                                        },
                                        for k in EventKind::ALL.iter().copied().filter(|k| *k != EventKind::AutomationRun) {
                                            option { value: "{event_kind_key(k)}", selected: k == event, "{k.label()}" }
                                        }
                                    }
                                }
                                Field { label: "Filter (optional)", hint: "Nur auslösen, wenn Spieler oder Meldung diesen Text enthält.",
                                    input {
                                        class: INPUT,
                                        value: "{filter.clone().unwrap_or_default()}",
                                        placeholder: "z. B. Spielername",
                                        oninput: move |e| {
                                            if let Trigger::Event { filter, .. } = &mut s.write().trigger {
                                                *filter = Some(e.value());
                                            }
                                        },
                                    }
                                }
                            }
                        },
                        Trigger::Interval { minutes } => rsx! {
                            div { class: "flex flex-wrap items-end gap-3",
                                Field { label: "Alle … Minuten", class: "w-40",
                                    input {
                                        class: INPUT,
                                        r#type: "number",
                                        min: "1",
                                        value: "{minutes}",
                                        oninput: move |e| s.write().trigger = Trigger::Interval { minutes: e.value().parse().unwrap_or(0) },
                                    }
                                }
                                for (m, l) in [(30u32, "30 Min"), (60, "1 Std"), (360, "6 Std"), (720, "12 Std")] {
                                    Button {
                                        key: "{m}",
                                        size: ButtonSize::Sm,
                                        variant: if minutes == m { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                                        onclick: move |_| s.write().trigger = Trigger::Interval { minutes: m },
                                        "{l}"
                                    }
                                }
                            }
                            p { class: "text-xs text-muted-foreground", "Läuft nur, solange der Server online ist – ausser die Aktionen starten den Server oder erstellen ein Backup." }
                        },
                        Trigger::Daily { hour, minute } => rsx! {
                            Field { label: "Uhrzeit", hint: "Ortszeit des Panels.", class: "w-40",
                                input {
                                    class: INPUT,
                                    r#type: "time",
                                    value: "{hour:02}:{minute:02}",
                                    oninput: move |e| {
                                        let v = e.value();
                                        let mut it = v.split(':');
                                        let h = it.next().and_then(|x| x.parse::<u8>().ok()).unwrap_or(0).min(23);
                                        let m = it.next().and_then(|x| x.parse::<u8>().ok()).unwrap_or(0).min(59);
                                        s.write().trigger = Trigger::Daily { hour: h, minute: m };
                                    },
                                }
                            }
                        },
                    }
                }

                // Aktionen
                div { class: "flex flex-col gap-3",
                    div { class: "flex flex-wrap items-center justify-between gap-2",
                        h3 { class: "text-sm font-semibold", "Was passiert?" }
                        select {
                            class: "{SELECT} w-auto",
                            value: "{add_kind}",
                            onchange: move |e| {
                                if let Some(a) = new_action(&e.value()) {
                                    s.write().actions.push(a);
                                }
                                add_kind.set(String::new());
                            },
                            option { value: "", selected: true, "+ Aktion hinzufügen …" }
                            for (k, l) in ACTION_KINDS.iter() {
                                option { value: "{k}", "{l}" }
                            }
                        }
                    }
                    if count == 0 {
                        div { class: "rounded-lg border border-dashed px-4 py-6 text-center text-sm text-muted-foreground",
                            "Noch keine Aktion. Wähle oben rechts eine aus – sie werden der Reihe nach ausgeführt."
                        }
                    }
                    for (i, act) in cur.actions.iter().cloned().enumerate() {
                        ActionRow {
                            key: "{i}",
                            index: i,
                            count,
                            action: act,
                            game,
                            is_event,
                            onchange: move |a: Action| {
                                if let Some(x) = s.write().actions.get_mut(i) {
                                    *x = a;
                                }
                            },
                            onmove: move |up: bool| {
                                let mut w = s.write();
                                if up && i > 0 {
                                    w.actions.swap(i, i - 1);
                                } else if !up && i + 1 < w.actions.len() {
                                    w.actions.swap(i, i + 1);
                                }
                            },
                            onremove: move |_| {
                                let mut w = s.write();
                                if i < w.actions.len() {
                                    w.actions.remove(i);
                                }
                            },
                        }
                    }
                }
                button { r#type: "submit", class: "hidden" }
            }
        }
    }
}

#[component]
fn ActionRow(
    index: usize,
    count: usize,
    action: Action,
    game: Game,
    is_event: bool,
    onchange: EventHandler<Action>,
    onmove: EventHandler<bool>,
    onremove: EventHandler<()>,
) -> Element {
    let placeholders = if is_event { "Platzhalter: {player} {message} {server}" } else { "Platzhalter: {server}" };
    let cmd_example = if game == Game::Minecraft { "z. B. give {player} minecraft:bread 5" } else { "z. B. /promote {player}" };
    let body = match action.clone() {
        Action::Command { command } => rsx! {
            Field { label: "Befehl", hint: placeholders,
                input {
                    class: "{INPUT} font-mono",
                    value: "{command}",
                    placeholder: cmd_example,
                    oninput: move |e| onchange.call(Action::Command { command: e.value() }),
                }
            }
        },
        Action::Broadcast { message } => rsx! {
            Field { label: "Nachricht", hint: placeholders,
                input {
                    class: INPUT,
                    value: "{message}",
                    placeholder: "z. B. Neustart in 60 Sekunden!",
                    oninput: move |e| onchange.call(Action::Broadcast { message: e.value() }),
                }
            }
        },
        Action::Start => rsx! { p { class: "text-sm text-muted-foreground", "Startet den Server, falls er nicht läuft." } },
        Action::Stop => rsx! { p { class: "text-sm text-muted-foreground", "Stoppt den Server sauber und wartet, bis er aus ist." } },
        Action::Restart => rsx! { p { class: "text-sm text-muted-foreground", "Startet den Server neu." } },
        Action::Backup { worlds_only } => rsx! {
            CheckRow {
                checked: worlds_only,
                label: "Nur Welten sichern",
                hint: "Sonst wird der ganze Server-Ordner gesichert.",
                onchange: move |v| onchange.call(Action::Backup { worlds_only: v }),
            }
        },
        Action::Webhook { url, discord } => {
            let url2 = url.clone();
            rsx! {
                div { class: "flex flex-col gap-3",
                    Field { label: "Adresse",
                        input {
                            class: "{INPUT} font-mono",
                            r#type: "url",
                            value: "{url}",
                            placeholder: "https://discord.com/api/webhooks/…",
                            oninput: move |e| onchange.call(Action::Webhook { url: e.value(), discord }),
                        }
                    }
                    CheckRow {
                        checked: discord,
                        label: "Discord-Format",
                        hint: "Sendet die Meldung als Discord-Nachricht. Sonst JSON mit Server, Ereignis und Text.",
                        onchange: move |v| onchange.call(Action::Webhook { url: url2.clone(), discord: v }),
                    }
                }
            }
        }
        Action::Delay { seconds } => rsx! {
            Field { label: "Sekunden warten", class: "w-40",
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "1",
                    value: "{seconds}",
                    oninput: move |e| onchange.call(Action::Delay { seconds: e.value().parse().unwrap_or(0) }),
                }
            }
        },
    };
    rsx! {
        div { class: "rounded-lg border bg-background/40 p-3",
            div { class: "mb-2 flex items-center gap-2",
                span { class: "flex size-6 items-center justify-center rounded-md bg-muted text-xs font-medium tabular-nums", "{index + 1}" }
                span { class: "text-muted-foreground", {action_icon(&action)} }
                span { class: "flex-1 text-sm font-medium", "{action_label(&action)}" }
                Button {
                    size: ButtonSize::IconXs,
                    variant: ButtonVariant::Ghost,
                    title: "Nach oben",
                    aria_label: "Nach oben",
                    disabled: index == 0,
                    onclick: move |_| onmove.call(true),
                    icons::ArrowUp { class: "size-3.5" }
                }
                Button {
                    size: ButtonSize::IconXs,
                    variant: ButtonVariant::Ghost,
                    title: "Nach unten",
                    aria_label: "Nach unten",
                    disabled: index + 1 >= count,
                    onclick: move |_| onmove.call(false),
                    icons::ArrowDown { class: "size-3.5" }
                }
                Button {
                    size: ButtonSize::IconXs,
                    variant: ButtonVariant::Ghost,
                    title: "Entfernen",
                    aria_label: "Entfernen",
                    onclick: move |_| onremove.call(()),
                    icons::X { class: "size-3.5" }
                }
            }
            {body}
        }
    }
}
