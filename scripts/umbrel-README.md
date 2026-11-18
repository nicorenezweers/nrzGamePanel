# nrzGamePanel – Umbrel-Paket

Nur fertige Dateien für die App **nrz-gamepanel** im nrzStore (der Quellcode liegt im Repo
`nicorenezweers/nrzGamePanel`). Erzeugt von `cargo xtask umbrel-release`.

| Datei | Inhalt |
|-------|--------|
| `nrz-gamepanel-amd64.gz`, `nrz-gamepanel-arm64.gz` | Panel-Programm (Linux, glibc) |
| `web.tar.gz` | Weboberfläche (Dioxus/WASM) |
| `SHA256SUMS` | Prüfsummen, geprüft von `hooks/pre-start` |
| `package/` | Store-Dateien (umbrel-app.yml, docker-compose.yml, Icon, Hooks) |
| `nrzStore.json` | nrzGit überträgt Paket und Version automatisch in den nrzStore |
