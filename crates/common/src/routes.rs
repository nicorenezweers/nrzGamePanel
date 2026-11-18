//! Alle HTTP-Routen (Vertrag zwischen Backend und Frontend).
//!
//! Authentifizierung: Sitzungs-Cookie `nrzgp_session` (Browser) oder `Authorization: Bearer <token>`
//! (API-Token, MCP). Fehler: HTTP-Status + [`crate::ApiError`] als JSON.
//!
//! | Methode | Pfad | Body → Antwort |
//! |---|---|---|
//! | GET | `/api/auth/me` | → `AuthState` (ohne Anmeldung erreichbar) |
//! | POST | `/api/auth/setup` | `Credentials` → `AuthState` (nur ohne Konto) |
//! | POST | `/api/auth/login` | `Credentials` → `AuthState` |
//! | POST | `/api/auth/logout` | → `Ok` |
//! | POST | `/api/auth/password` | `ChangePassword` → `Ok` |
//! | GET/POST | `/api/tokens` | → `Vec<ApiToken>` / `CreateToken` → `CreatedToken` |
//! | DELETE | `/api/tokens/{tid}` | → `Ok` |
//! | GET | `/api/system` | → `SystemInfo` |
//! | GET | `/api/versions/{game}/{flavor}?game_version=` | → `VersionList` |
//! | GET | `/api/check-port?port=&game=` | → `PortCheck` |
//! | GET | `/api/events?limit=` | → `Vec<GameEvent>` (alle Server, neueste zuerst) |
//! | GET | `/api/servers` | → `Vec<ServerSummary>` |
//! | POST | `/api/servers` | `CreateServer` → `ServerSummary` (Installation läuft im Hintergrund, Fortschritt in der Konsole) |
//! | GET/PATCH/DELETE | `/api/servers/{id}` | → `ServerSummary` / `UpdateServer` → `ServerSummary` / `?delete_files=true` → `Ok` |
//! | POST | `/api/servers/{id}/power` | `PowerRequest` → `Ok` |
//! | POST | `/api/servers/{id}/command` | `CommandRequest` → `CommandResponse` |
//! | POST | `/api/servers/{id}/eula` | → `Ok` (eula=true) |
//! | POST | `/api/servers/{id}/rcon/enable` | → `ServerSummary` |
//! | GET (WS) | `/api/servers/{id}/ws` | `WsMessage` ⇄ `WsClientMessage` |
//! | GET | `/api/servers/{id}/console?limit=` | → `Vec<ConsoleLine>` |
//! | GET | `/api/servers/{id}/events?limit=` | → `Vec<GameEvent>` |
//! | GET | `/api/servers/{id}/files?path=` | → `DirListing` |
//! | GET/PUT | `/api/servers/{id}/files/content?path=` | → `FileContent` / `WriteFile` → `Ok` |
//! | GET | `/api/servers/{id}/files/download?path=` | → Datei (Ordner als .zip) |
//! | POST | `/api/servers/{id}/files/upload?path=` | multipart (Feld `file`, mehrfach) → `Ok` |
//! | POST | `/api/servers/{id}/files/mkdir` | `PathRequest` → `Ok` |
//! | POST | `/api/servers/{id}/files/touch` | `PathRequest` → `Ok` |
//! | POST | `/api/servers/{id}/files/rename` | `RenameRequest` → `Ok` |
//! | POST | `/api/servers/{id}/files/delete` | `PathsRequest` → `Ok` |
//! | POST | `/api/servers/{id}/files/extract` | `PathRequest` (zip/tar.gz) → `Ok` |
//! | POST | `/api/servers/{id}/files/archive` | `PathsRequest` → `Ok` (erstellt `archive-<zeit>.zip`) |
//! | GET/POST | `/api/servers/{id}/backups` | → `Vec<Backup>` / `CreateBackup` → `Backup` |
//! | POST | `/api/servers/{id}/backups/{name}/restore` | → `Ok` (Server muss gestoppt sein) |
//! | GET | `/api/servers/{id}/backups/{name}/download` | → Datei |
//! | DELETE | `/api/servers/{id}/backups/{name}` | → `Ok` |
//! | GET/POST | `/api/servers/{id}/automations` | → `Vec<Automation>` / `SaveAutomation` → `Automation` |
//! | PUT/DELETE | `/api/servers/{id}/automations/{aid}` | `SaveAutomation` → `Automation` / → `Ok` |
//! | POST | `/api/servers/{id}/automations/{aid}/run` | → `Ok` |
//! | GET | `/api/fs/browse?path=` | → `HostDirListing` |
//! | POST | `/api/import/analyze` | `AnalyzeRequest` → `ImportAnalysis` |
//! | POST | `/api/import/scan` | `ScanRequest` → `Vec<ImportAnalysis>` |
//! | POST | `/api/import` | `ImportRequest` → `ServerSummary` |
//! | GET/PUT | `/api/servers/{id}/mc/properties` | → `Properties` / `Properties` → `Ok` |
//! | GET | `/api/servers/{id}/mc/players` | → `PlayersOverview` |
//! | GET | `/api/servers/{id}/mc/players/{uuid}` | → `PlayerDetail` |
//! | POST | `/api/servers/{id}/mc/players/{uuid}/inventory` | `InventoryOp` → `PlayerDetail` |
//! | POST | `/api/servers/{id}/mc/players/{name}/action` | `PlayerAction` → `CommandResponse` |
//! | POST | `/api/servers/{id}/mc/whitelist` | `ToggleRequest` → `Ok` |
//! | GET | `/api/servers/{id}/mc/worlds` | → `Vec<WorldInfo>` |
//! | GET | `/api/servers/{id}/mc/map/{world}/regions?dim=` | → `MapRegions` |
//! | GET | `/api/servers/{id}/mc/map/{world}/tile/{rx}/{rz}?dim=` | → PNG 512×512 (1 px = 1 Block) |
//! | GET | `/api/servers/{id}/mc/map/players` | → `Vec<MapPlayer>` |
//! | GET | `/api/servers/{id}/mc/items` | → `ItemCatalog` |
//! | GET | `/api/servers/{id}/mc/texture/{item}` | → PNG (Item-/Blocktextur aus dem offiziellen Client-Jar) |
//! | GET | `/api/mc/head/{player}` | → PNG (Spielerkopf, zwischengespeichert) |
//! | GET | `/api/servers/{id}/mc/addons` | → `AddonList` |
//! | POST | `/api/servers/{id}/mc/addons/toggle` | `AddonToggle` → `Ok` |
//! | DELETE | `/api/servers/{id}/mc/addons/{file}` | → `Ok` |
//! | GET | `/api/servers/{id}/mc/modrinth/search?q=` | → `Vec<ModrinthHit>` |
//! | POST | `/api/servers/{id}/mc/modrinth/install` | `ModrinthInstall` → `Addon` |
//! | GET | `/api/servers/{id}/factorio/saves` | → `FactorioSaves` |
//! | POST | `/api/servers/{id}/factorio/saves` | `SaveName` → `Ok` (neue Karte erzeugen) |
//! | POST | `/api/servers/{id}/factorio/saves/select` | `SaveName` → `Ok` |
//! | GET | `/api/servers/{id}/factorio/mods` | → `FactorioMods` |
//! | POST | `/api/servers/{id}/factorio/mods/toggle` | `AddonToggle` → `Ok` |
//! | GET/PUT | `/api/servers/{id}/factorio/settings` | → `FactorioSettings` / `FactorioSettings` → `Ok` |
//! | GET/PUT | `/api/servers/{id}/factorio/admins` | → `NameList` / `NameList` → `Ok` |
//! | POST | `/mcp` | MCP (Streamable HTTP, JSON-RPC 2.0), Bearer-Token |

/// URL-Pfad eines Servers, z. B. `server("abc", "files")` → `/api/servers/abc/files`.
pub fn server(id: &str, rest: &str) -> String {
    if rest.is_empty() { format!("/api/servers/{id}") } else { format!("/api/servers/{id}/{rest}") }
}

/// Prozent-Kodierung für Query-Parameter.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
