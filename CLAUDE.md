# nrzGamePanel

Spielserver-Panel (Minecraft, Factorio), nur Rust. Workspace:

- `crates/common` – API-Vertrag (Typen, alle Routen in `routes.rs`). Änderungen hier betreffen Backend und Web.
- `crates/server` – Backend (Axum, tokio). `process.rs` = Prozess/Konsole/RCON, `servers.rs` = Routen,
  `minecraft/`, `import/`, `install/`, `factorio.rs` = Spiel-Integrationen, `mcp.rs` = MCP-Server.
- `crates/web` – Oberfläche (Dioxus 0.7 web, nrzUI-Bausteine in `src/components/ui`, nrz-Design: DESIGN.md im nrzUI-Repo).
  Seiten in `src/pages`, Live-Daten eines Servers über `layout::use_server()` (WebSocket).

Regeln: Oberfläche Deutsch (Schweizer Schreibweise, ss statt ß), nur Theme-Farben (`bg-card`, `text-muted-foreground`,
Zustände `success/warning/destructive/info`), kein JavaScript. Prüfen: `cargo check -p nrz-gamepanel`,
`cargo check -p nrz-gamepanel-web --target wasm32-unknown-unknown`, `cargo test -p nrz-gamepanel`.
Läuft im Docker-Host-Modus (Spiel-Ports direkt auf dem Host). Panel-Port 8701 (Store: app_proxy 8700).
