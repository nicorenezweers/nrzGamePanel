//! Automatisierungen: Ereignis-, Intervall- und Tages-Auslöser → Aktionen.

use std::time::Duration;

use axum::{
    Json,
    extract::{Path as AxPath, State},
};
use chrono::Timelike;
use nrz_gp_common::{Action, Automation, EventKind, Game, GameEvent, Ok as OkResp, SaveAutomation, Trigger};

use crate::{
    auth::Authed,
    backups,
    error::{AppError, AppResult},
    state::AppState,
    store::{new_id, now},
};

pub async fn list(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>) -> AppResult<Json<Vec<Automation>>> {
    app.server(&id).await?;
    Ok(Json(app.store.read(|s| s.automations.iter().filter(|a| a.server_id == id).cloned().collect()).await))
}

fn validate(s: &SaveAutomation) -> AppResult<()> {
    if s.name.trim().is_empty() {
        return Err(AppError::bad("Name fehlt"));
    }
    if s.actions.is_empty() {
        return Err(AppError::bad("Mindestens eine Aktion nötig"));
    }
    match s.trigger {
        Trigger::Interval { minutes } if minutes == 0 => return Err(AppError::bad("Intervall muss mindestens 1 Minute sein")),
        Trigger::Daily { hour, minute } if hour > 23 || minute > 59 => return Err(AppError::bad("Ungültige Uhrzeit")),
        _ => {}
    }
    Ok(())
}

pub async fn create(State(app): State<AppState>, _a: Authed, AxPath(id): AxPath<String>, Json(s): Json<SaveAutomation>) -> AppResult<Json<Automation>> {
    app.server(&id).await?;
    validate(&s)?;
    let a = Automation {
        id: new_id(6),
        server_id: id,
        name: s.name.trim().into(),
        enabled: s.enabled,
        trigger: s.trigger,
        actions: s.actions,
        last_run: None,
    };
    app.store.update(|st| st.automations.push(a.clone())).await?;
    Ok(Json(a))
}

pub async fn update(
    State(app): State<AppState>,
    _a: Authed,
    AxPath((id, aid)): AxPath<(String, String)>,
    Json(s): Json<SaveAutomation>,
) -> AppResult<Json<Automation>> {
    validate(&s)?;
    let r = app
        .store
        .update(|st| {
            st.automations.iter_mut().find(|a| a.id == aid && a.server_id == id).map(|a| {
                a.name = s.name.trim().into();
                a.enabled = s.enabled;
                a.trigger = s.trigger;
                a.actions = s.actions;
                a.clone()
            })
        })
        .await?;
    r.map(Json).ok_or_else(|| AppError::not_found("Automatisierung nicht gefunden"))
}

pub async fn delete(State(app): State<AppState>, _a: Authed, AxPath((id, aid)): AxPath<(String, String)>) -> AppResult<Json<OkResp>> {
    app.store.update(|st| st.automations.retain(|a| !(a.id == aid && a.server_id == id))).await?;
    Ok(Json(OkResp { ok: true }))
}

pub async fn run_now(State(app): State<AppState>, _a: Authed, AxPath((id, aid)): AxPath<(String, String)>) -> AppResult<Json<OkResp>> {
    let a = app.store.read(|st| st.automations.iter().find(|a| a.id == aid && a.server_id == id).cloned()).await;
    let a = a.ok_or_else(|| AppError::not_found("Automatisierung nicht gefunden"))?;
    tokio::spawn(run(app.clone(), a, None));
    Ok(Json(OkResp { ok: true }))
}

fn fill(template: &str, ev: Option<&GameEvent>, server: &str) -> String {
    template
        .replace("{player}", ev.and_then(|e| e.player.as_deref()).unwrap_or(""))
        .replace("{message}", ev.map_or("", |e| e.message.as_str()))
        .replace("{server}", server)
}

/// Aktionen einer Automatisierung nacheinander ausführen.
pub async fn run(app: AppState, a: Automation, ev: Option<GameEvent>) {
    let Ok((cfg, rt)) = app.server(&a.server_id).await else { return };
    let _ = app.store.update(|st| {
        if let Some(x) = st.automations.iter_mut().find(|x| x.id == a.id) {
            x.last_run = Some(now());
        }
    })
    .await;
    rt.panel(format!("⚙ Automatisierung „{}“ wird ausgeführt", a.name));
    for action in &a.actions {
        let res: AppResult<()> = async {
            match action {
                Action::Command { command } => {
                    rt.command(&app, &fill(command, ev.as_ref(), &cfg.name)).await?;
                }
                Action::Broadcast { message } => {
                    let msg = fill(message, ev.as_ref(), &cfg.name);
                    let cmd = match cfg.game {
                        Game::Minecraft => format!("say {msg}"),
                        Game::Factorio => format!("silent-command game.print(\"{}\")", msg.replace('"', "'")),
                    };
                    rt.command(&app, &cmd).await?;
                }
                Action::Start => {
                    if !rt.status().is_online() {
                        rt.start(&app).await?;
                    }
                }
                Action::Stop => rt.stop_and_wait(&app).await?,
                Action::Restart => rt.restart(&app).await?,
                Action::Backup { worlds_only } => {
                    backups::create_backup(&app, &a.server_id, *worlds_only).await?;
                }
                Action::Webhook { url, discord } => {
                    let text = match &ev {
                        Some(e) => format!("[{}] {}: {}", cfg.name, e.kind.label(), e.message),
                        None => format!("[{}] Automatisierung „{}“", cfg.name, a.name),
                    };
                    let body = if *discord {
                        serde_json::json!({ "content": text, "username": "nrzGamePanel" })
                    } else {
                        serde_json::json!({ "server": cfg.name, "server_id": cfg.id, "automation": a.name, "event": ev, "text": text })
                    };
                    app.http.post(url).json(&body).timeout(Duration::from_secs(10)).send().await?.error_for_status()?;
                }
                Action::Delay { seconds } => tokio::time::sleep(Duration::from_secs(u64::from(*seconds))).await,
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            rt.panel(format!("⚙ Aktion fehlgeschlagen: {e}"));
        }
    }
    if ev.as_ref().is_none_or(|e| e.kind != EventKind::AutomationRun) {
        app.events.emit(&a.server_id, EventKind::AutomationRun, None, format!("„{}“ ausgeführt", a.name));
    }
}

/// Hintergrund: Ereignisse beobachten und Zeitpläne prüfen (minütlich).
pub fn spawn_scheduler(app: AppState) {
    // Ereignis-Auslöser
    let a1 = app.clone();
    tokio::spawn(async move {
        let mut rx = a1.events.tx.subscribe();
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            if ev.kind == EventKind::AutomationRun {
                continue;
            }
            let matching: Vec<Automation> = a1
                .store
                .read(|st| {
                    st.automations
                        .iter()
                        .filter(|a| a.enabled && a.server_id == ev.server_id)
                        .filter(|a| match &a.trigger {
                            Trigger::Event { event, filter } => {
                                *event == ev.kind
                                    && filter.as_ref().filter(|f| !f.is_empty()).is_none_or(|f| {
                                        let f = f.to_lowercase();
                                        ev.message.to_lowercase().contains(&f)
                                            || ev.player.as_ref().is_some_and(|p| p.to_lowercase().contains(&f))
                                    })
                            }
                            _ => false,
                        })
                        .cloned()
                        .collect()
                })
                .await;
            for a in matching {
                tokio::spawn(run(a1.clone(), a, Some(ev.clone())));
            }
        }
    });
    // Zeit-Auslöser
    tokio::spawn(async move {
        let mut minute: u64 = 0;
        loop {
            // auf den Beginn der nächsten Minute warten
            let secs = chrono::Local::now().second();
            tokio::time::sleep(Duration::from_secs(u64::from(60 - secs.min(59)))).await;
            minute += 1;
            let t = chrono::Local::now();
            let autos = app.store.read(|st| st.automations.iter().filter(|a| a.enabled).cloned().collect::<Vec<_>>()).await;
            for a in autos {
                let due = match a.trigger {
                    Trigger::Daily { hour, minute: m } => t.hour() == u32::from(hour) && t.minute() == u32::from(m),
                    Trigger::Interval { minutes } => {
                        let running = app.runtime(&a.server_id).await.status().is_online();
                        let starts_server = a.actions.iter().any(|x| matches!(x, Action::Start | Action::Backup { .. }));
                        minutes > 0 && minute % u64::from(minutes) == 0 && (running || starts_server)
                    }
                    Trigger::Event { .. } => false,
                };
                if due {
                    tokio::spawn(run(app.clone(), a, None));
                }
            }
        }
    });
}
