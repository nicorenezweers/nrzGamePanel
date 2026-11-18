//! Seiten. Jede Datei = ein Bereich.

mod addons;
mod automations;
mod backups;
mod console;
mod create;
mod dashboard;
mod factorio;
mod files;
mod import;
mod integrations;
mod map;
mod overview;
mod panel_settings;
mod players;
mod server_settings;

pub use addons::ServerAddons;
pub use automations::ServerAutomations;
pub use backups::ServerBackups;
pub use console::ServerConsole;
pub use create::CreateServer;
pub use dashboard::Dashboard;
pub use factorio::FactorioPage;
pub use files::ServerFiles;
pub use import::ImportServer;
pub use integrations::Integrations;
pub use map::ServerMap;
pub use overview::ServerOverview;
pub use panel_settings::PanelSettings;
pub use players::{PlayerPage, ServerPlayers};
pub use server_settings::ServerSettings;
