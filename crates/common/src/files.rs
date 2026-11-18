//! Datei-Explorer (innerhalb eines Server-Ordners) und Host-Ordnerauswahl (Import).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub name: String,
    /// Pfad relativ zum Server-Ordner, mit `/` getrennt, ohne führenden `/`.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix-Zeit in Sekunden.
    pub modified: i64,
}

/// `GET /api/servers/{id}/files?path=`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// `GET /api/servers/{id}/files/content?path=`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    /// Datei ist binär oder zu gross → `content` leer, nur Download.
    pub binary: bool,
    pub size: u64,
}

/// `PUT /api/servers/{id}/files/content`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WriteFile {
    pub path: String,
    pub content: String,
}

/// `POST /api/servers/{id}/files/mkdir` bzw. `…/touch`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PathRequest {
    pub path: String,
}

/// `POST /api/servers/{id}/files/rename` (auch Verschieben)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RenameRequest {
    pub from: String,
    pub to: String,
}

/// `POST /api/servers/{id}/files/delete` und `…/archive` (zip erstellen)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PathsRequest {
    pub paths: Vec<String>,
}

/// Host-Ordner für die Import-Auswahl: `GET /api/fs/browse?path=`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostDirEntry {
    pub name: String,
    /// Absoluter Pfad.
    pub path: String,
    /// Sieht nach einem Spielserver aus (schneller Check).
    pub looks_like_server: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostDirListing {
    pub path: String,
    pub parent: Option<String>,
    pub dirs: Vec<HostDirEntry>,
    /// Startpunkte (Import-Wurzeln), z. B. `/srv`, `/import`.
    pub roots: Vec<String>,
}
