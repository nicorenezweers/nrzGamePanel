//! Gemeinsame API-Typen von nrzGamePanel.
//!
//! Alles, was zwischen Backend (`nrz-gamepanel`, Axum) und Frontend (`nrz-gamepanel-web`, Dioxus)
//! über HTTP/WebSocket geht, ist hier definiert. Alle Routen liegen unter `/api` (siehe [`routes`]).

pub mod automation;
pub mod events;
pub mod factorio;
pub mod files;
pub mod import;
pub mod minecraft;
pub mod routes;
pub mod server;
pub mod system;

pub use automation::*;
pub use events::*;
pub use factorio::*;
pub use files::*;
pub use import::*;
pub use minecraft::*;
pub use server::*;
pub use system::*;

use serde::{Deserialize, Serialize};

/// Version des Panels (Cargo-Version von `nrz-gp-common`).
pub const PANEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Fehlerantwort jeder Route (HTTP-Status ≠ 2xx).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiError {
    pub error: String,
}

/// Einfache Erfolgsantwort ohne Daten.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Ok {
    pub ok: bool,
}

/* ------------------------------------------------------------------ */
/*                              Auth                                  */
/* ------------------------------------------------------------------ */

/// `GET /api/auth/me`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AuthState {
    /// Noch kein Konto angelegt → Ersteinrichtung zeigen.
    pub setup_required: bool,
    /// Angemeldeter Benutzer (None = nicht angemeldet).
    pub user: Option<String>,
}

/// `POST /api/auth/setup` und `POST /api/auth/login`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// `POST /api/auth/password`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChangePassword {
    pub current: String,
    pub new: String,
}

/// API-Token (für MCP und Skripte). Der Klartext wird nur beim Anlegen einmal gezeigt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    /// Erste Zeichen zur Wiedererkennung, z. B. `nrzgp_a1b2…`.
    pub prefix: String,
    pub created_at: i64,
    pub last_used: Option<i64>,
}

/// `POST /api/tokens`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreateToken {
    pub name: String,
}

/// Antwort auf `POST /api/tokens`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreatedToken {
    pub token: ApiToken,
    /// Klartext, nur dieses eine Mal sichtbar.
    pub secret: String,
}
