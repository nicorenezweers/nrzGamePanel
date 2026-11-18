//! Globaler Zustand: Anmeldung, Serverliste, Meldungen (Toasts).

use dioxus::prelude::*;
use nrz_gp_common::{AuthState, ServerSummary, SystemInfo};

/// None = noch nicht geladen.
pub static AUTH: GlobalSignal<Option<AuthState>> = Signal::global(|| None);
/// Serverliste (Seitenleiste, Übersicht), alle paar Sekunden aktualisiert.
pub static SERVERS: GlobalSignal<Vec<ServerSummary>> = Signal::global(Vec::new);
pub static SERVERS_LOADED: GlobalSignal<bool> = Signal::global(|| false);
pub static SYSTEM: GlobalSignal<Option<SystemInfo>> = Signal::global(|| None);
pub static TOASTS: GlobalSignal<Vec<Toast>> = Signal::global(Vec::new);

#[derive(Clone, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

pub fn on_unauthorized() {
    let setup_required = AUTH.peek().as_ref().is_some_and(|a| a.setup_required);
    *AUTH.write() = Some(AuthState { setup_required, user: None });
}

fn push_toast(text: String, error: bool) {
    let id = js_sys::Date::now() as u64 ^ (js_sys::Math::random() * 1e9) as u64;
    TOASTS.write().push(Toast { id, text, error });
    dioxus::dioxus_core::spawn_forever(async move {
        gloo_timers::future::TimeoutFuture::new(if error { 6000 } else { 3500 }).await;
        TOASTS.write().retain(|t| t.id != id);
    });
}

/// Erfolgsmeldung unten rechts.
pub fn toast(text: impl Into<String>) {
    push_toast(text.into(), false);
}

/// Fehlermeldung unten rechts.
pub fn toast_err(text: impl Into<String>) {
    push_toast(text.into(), true);
}

/// Serverliste neu laden.
pub async fn refresh_servers() {
    if let Ok(list) = crate::api::get::<Vec<ServerSummary>>("/api/servers").await {
        *SERVERS.write() = list;
        *SERVERS_LOADED.write() = true;
    }
}

/// Server aus der Liste (falls geladen).
pub fn server(id: &str) -> Option<ServerSummary> {
    SERVERS.read().iter().find(|s| s.config.id == id).cloned()
}
