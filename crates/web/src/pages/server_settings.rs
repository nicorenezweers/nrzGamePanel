//! Server-Einstellungen: Allgemein, Start, RCON, server.properties (Minecraft), Gefahrenzone.

use dioxus::prelude::*;
use nrz_gp_common::{Flavor, Game, LaunchKind, PortCheck, PowerAction, Properties, ServerConfig, ServerSummary, UpdateServer, routes};

use crate::{
    Route, api,
    components::{
        CheckRow, CopyField, ErrorBox, INPUT, Loading, Modal, Notice, SELECT, TEXTAREA, Tag, Toggle,
        ui::{
            app_shell::{SettingsGroup, SettingsRow},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::{power, use_server},
    store::{SYSTEM, refresh_servers, toast, toast_err},
};

/// Empfohlene JVM-Flags für Minecraft-Server (Aikar).
const AIKAR_FLAGS: &str = "-XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200 -XX:+UnlockExperimentalVMOptions -XX:+DisableExplicitGC -XX:+AlwaysPreTouch -XX:G1NewSizePercent=30 -XX:G1MaxNewSizePercent=40 -XX:G1HeapRegionSize=8M -XX:G1ReservePercent=20 -XX:G1HeapWastePercent=5 -XX:G1MixedGCCountTarget=4 -XX:InitiatingHeapOccupancyPercent=15 -XX:G1MixedGCLiveThresholdPercent=90 -XX:G1RSetUpdatingPauseTimePercent=5 -XX:SurvivorRatio=32 -XX:+PerfDisableSharedMem -XX:MaxTenuringThreshold=1 -Dusing.aikars.flags=https://mcflags.emc.gs -Daikars.new.flags=true";

const MC_FLAVORS: &[Flavor] = &[
    Flavor::Paper,
    Flavor::Purpur,
    Flavor::Folia,
    Flavor::Spigot,
    Flavor::Bukkit,
    Flavor::Vanilla,
    Flavor::Fabric,
    Flavor::Quilt,
    Flavor::Forge,
    Flavor::NeoForge,
    Flavor::Unknown,
];

const TEXT_W: &str = "w-full sm:w-72";

#[component]
pub fn ServerSettings(id: String) -> Element {
    let ctx = use_server();
    let Some(sum) = ctx.summary.read().clone() else { return rsx! { Loading {} } };
    let is_mc = sum.config.game == Game::Minecraft;
    rsx! {
        div { class: "mx-auto max-w-7xl p-4 md:p-8",
            div { class: "flex max-w-3xl flex-col gap-8",
                ConfigForm { key: "cfg-{id}", summary: sum.clone() }
                RconGroup { summary: sum.clone() }
                if is_mc {
                    PropertiesEditor { key: "props-{id}", id: id.clone() }
                }
                DangerZone { summary: sum }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                        Allgemein + Start                           */
/* ------------------------------------------------------------------ */

#[derive(Clone, PartialEq)]
struct Form {
    name: String,
    flavor: Flavor,
    version: String,
    port: String,
    auto_start: bool,
    auto_restart: bool,
    launch_kind: &'static str,
    launch_path: String,
    factorio_save: Option<String>,
    /// Leer = automatisch.
    java: String,
    memory_mb: u32,
    jvm_args: String,
    extra_args: String,
}

impl Form {
    fn from_config(c: &ServerConfig) -> Self {
        let (launch_kind, launch_path, factorio_save) = match &c.launch {
            LaunchKind::Jar { jar } => ("jar", jar.clone(), None),
            LaunchKind::ArgsFile { args_file } => ("args_file", args_file.clone(), None),
            LaunchKind::Script { script } => ("script", script.clone(), None),
            LaunchKind::Factorio { binary, save } => ("factorio", binary.clone(), save.clone()),
        };
        Self {
            name: c.name.clone(),
            flavor: c.flavor,
            version: c.version.clone().unwrap_or_default(),
            port: c.port.to_string(),
            auto_start: c.auto_start,
            auto_restart: c.auto_restart,
            launch_kind,
            launch_path,
            factorio_save,
            java: c.java_path.clone().unwrap_or_default(),
            memory_mb: c.memory_mb,
            jvm_args: c.jvm_args.clone(),
            extra_args: c.extra_args.clone(),
        }
    }

    fn launch(&self) -> LaunchKind {
        let p = self.launch_path.trim().to_string();
        match self.launch_kind {
            "args_file" => LaunchKind::ArgsFile { args_file: p },
            "script" => LaunchKind::Script { script: p },
            "factorio" => LaunchKind::Factorio { binary: p, save: self.factorio_save.clone() },
            _ => LaunchKind::Jar { jar: p },
        }
    }

    fn validate(&self) -> Result<u16, String> {
        if self.name.trim().is_empty() {
            return Err("Der Name darf nicht leer sein.".into());
        }
        if self.launch_path.trim().is_empty() {
            return Err("Bitte eine Startdatei angeben.".into());
        }
        match self.port.trim().parse::<u16>() {
            Ok(p) if p > 0 => Ok(p),
            _ => Err("Der Port muss eine Zahl zwischen 1 und 65535 sein.".into()),
        }
    }

    /// Nur geänderte Felder.
    fn diff(&self, o: &Form) -> UpdateServer {
        let mut u = UpdateServer::default();
        if self.name.trim() != o.name.trim() {
            u.name = Some(self.name.trim().to_string());
        }
        if self.flavor != o.flavor {
            u.flavor = Some(self.flavor);
        }
        if self.version.trim() != o.version.trim() {
            u.version = Some(self.version.trim().to_string());
        }
        if self.port.trim() != o.port.trim() {
            u.port = self.port.trim().parse().ok();
        }
        if self.auto_start != o.auto_start {
            u.auto_start = Some(self.auto_start);
        }
        if self.auto_restart != o.auto_restart {
            u.auto_restart = Some(self.auto_restart);
        }
        if self.launch() != o.launch() {
            u.launch = Some(self.launch());
        }
        if self.java != o.java {
            u.java_path = Some(if self.java.is_empty() { None } else { Some(self.java.clone()) });
        }
        if self.memory_mb != o.memory_mb {
            u.memory_mb = Some(self.memory_mb);
        }
        if self.jvm_args.trim() != o.jvm_args.trim() {
            u.jvm_args = Some(self.jvm_args.trim().to_string());
        }
        if self.extra_args.trim() != o.extra_args.trim() {
            u.extra_args = Some(self.extra_args.trim().to_string());
        }
        u
    }

    /// Vorschau des Startbefehls.
    fn preview(&self, java: &str, uses_java: bool) -> String {
        let launch = self.launch();
        let base = launch.describe();
        let mut out = match base.strip_prefix("java ") {
            Some(rest) if uses_java => {
                let mut parts = vec![java.to_string(), format!("-Xms{0}M -Xmx{0}M", self.memory_mb)];
                if !self.jvm_args.trim().is_empty() {
                    parts.push(self.jvm_args.trim().to_string());
                }
                parts.push(rest.to_string());
                parts.push("nogui".into());
                parts.join(" ")
            }
            _ => base,
        };
        if !self.extra_args.trim().is_empty() {
            out.push(' ');
            out.push_str(self.extra_args.trim());
        }
        out
    }
}

fn fmt_gb(mb: u32) -> String {
    if mb % 1024 == 0 { format!("{} GB", mb / 1024) } else { format!("{:.1} GB", f64::from(mb) / 1024.0) }
}

fn launch_label(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "args_file" => ("Argumentdatei", "z. B. libraries/net/neoforged/neoforge/…/unix_args.txt"),
        "script" => ("Skript", "z. B. start.sh"),
        "factorio" => ("Programm", "z. B. bin/x64/factorio"),
        _ => ("Jar-Datei", "z. B. server.jar"),
    }
}

#[component]
fn ConfigForm(summary: ServerSummary) -> Element {
    let ctx = use_server();
    let mut orig = use_signal(|| Form::from_config(&summary.config));
    let mut form = use_signal(|| Form::from_config(&summary.config));
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let mut port_info = use_signal(|| None::<PortCheck>);

    let game = summary.config.game;
    let uses_java = game == Game::Minecraft;
    let id = summary.config.id.clone();

    // Port live prüfen, wenn er geändert wurde
    let port = use_memo(move || form.read().port.clone());
    use_effect(move || {
        let p = port();
        if p.trim() == orig.peek().port.trim() {
            port_info.set(None);
            return;
        }
        let Ok(n) = p.trim().parse::<u16>() else {
            port_info.set(None);
            return;
        };
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(300).await;
            if form.peek().port.trim() != n.to_string() {
                return;
            }
            let g = if game == Game::Factorio { "factorio" } else { "minecraft" };
            match api::get::<PortCheck>(&format!("/api/check-port?port={n}&game={g}")).await {
                Ok(c) => port_info.set(Some(c)),
                Err(_) => port_info.set(None),
            }
        });
    });

    let save = move |_| {
        let f = form();
        if let Err(e) = f.validate() {
            error.set(Some(e));
            return;
        }
        let update = f.diff(&orig.peek());
        error.set(None);
        saving.set(true);
        let id = id.clone();
        let mut summary_sig = ctx.summary;
        spawn(async move {
            match api::patch::<_, ServerSummary>(&routes::server(&id, ""), &update).await {
                Ok(s) => {
                    let nf = Form::from_config(&s.config);
                    orig.set(nf.clone());
                    form.set(nf);
                    port_info.set(None);
                    summary_sig.set(Some(s));
                    toast("Einstellungen gespeichert");
                    refresh_servers().await;
                }
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };

    let f = form();
    let dirty = f != orig();
    let (path_label, path_hint) = launch_label(f.launch_kind);
    let system = SYSTEM.read().clone();
    let javas = system.as_ref().map(|s| s.java.clone()).unwrap_or_default();
    let total_mb = system.as_ref().map_or(0, |s| (s.memory_total / 1024 / 1024) as u32);
    let free_mb = system.as_ref().map_or(0, |s| (s.memory_total.saturating_sub(s.memory_used) / 1024 / 1024) as u32);
    let max_mb = total_mb.clamp(16384, 65536).max(f.memory_mb);
    let java_now = summary.java.clone();
    let java_cmd = if f.java.is_empty() { java_now.clone().unwrap_or_else(|| "java".into()) } else { f.java.clone() };
    let preview = f.preview(&java_cmd, uses_java);
    let aikar_active = f.jvm_args.contains("aikars.new.flags");
    let launch_kinds: &[(&str, &str)] = if uses_java {
        &[("jar", "Jar-Datei"), ("args_file", "Argumentdatei (Forge/NeoForge)"), ("script", "Eigenes Skript")]
    } else {
        &[("factorio", "Factorio-Programm"), ("script", "Eigenes Skript")]
    };

    rsx! {
        SettingsGroup { title: "Allgemein",
            SettingsRow { label: "Name",
                input {
                    class: "{INPUT} {TEXT_W}",
                    value: "{f.name}",
                    oninput: move |e| form.write().name = e.value(),
                }
            }
            SettingsRow { label: "Software",
                if uses_java {
                    select {
                        class: "{SELECT} {TEXT_W}",
                        onchange: move |e| {
                            if let Some(fl) = MC_FLAVORS.iter().find(|fl| fl.label() == e.value()) {
                                form.write().flavor = *fl;
                            }
                        },
                        for fl in MC_FLAVORS.iter() {
                            option { key: "{fl.label()}", value: "{fl.label()}", selected: *fl == f.flavor, "{fl.label()}" }
                        }
                    }
                } else {
                    Tag { "{f.flavor.label()}" }
                }
            }
            SettingsRow { label: "Version", hint: "Bestimmt die passende Java-Version. Die Startdatei wird dadurch nicht ausgetauscht.",
                input {
                    class: "{INPUT} {TEXT_W}",
                    value: "{f.version}",
                    placeholder: if uses_java { "z. B. 1.21.4" } else { "z. B. 2.0.28" },
                    oninput: move |e| form.write().version = e.value(),
                }
            }
            SettingsRow {
                label: "Port",
                hint: if game == Game::Factorio { "UDP, Standard 34197" } else { "TCP, Standard 25565" },
                div { class: "flex flex-col items-end gap-1",
                    input {
                        class: "{INPUT} w-32",
                        r#type: "number",
                        min: "1",
                        max: "65535",
                        value: "{f.port}",
                        oninput: move |e| form.write().port = e.value(),
                    }
                    if let Some(pi) = port_info() {
                        if pi.free {
                            span { class: "inline-flex items-center gap-1 text-xs text-success",
                                icons::CircleCheck { class: "size-3.5" }
                                "Port ist frei"
                            }
                        } else {
                            span { class: "inline-flex items-center gap-1 text-xs text-destructive",
                                icons::CircleAlert { class: "size-3.5" }
                                if let Some(u) = pi.used_by {
                                    "Belegt durch „{u}“"
                                } else {
                                    "Port ist belegt"
                                }
                            }
                        }
                    }
                }
            }
            SettingsRow { label: "Automatisch starten", hint: "Startet den Server, wenn das Panel startet.",
                Toggle {
                    checked: f.auto_start,
                    label: "Automatisch starten",
                    onchange: move |v| form.write().auto_start = v,
                }
            }
            SettingsRow { label: "Neustart nach Absturz", hint: "Startet den Server automatisch neu, wenn er abstürzt.",
                Toggle {
                    checked: f.auto_restart,
                    label: "Neustart nach Absturz",
                    onchange: move |v| form.write().auto_restart = v,
                }
            }
        }

        SettingsGroup { title: "Start",
            SettingsRow { label: "Startart",
                select {
                    class: "{SELECT} {TEXT_W}",
                    onchange: move |e| {
                        let v = e.value();
                        if let Some((k, _)) = launch_kinds.iter().find(|(k, _)| *k == v) {
                            let mut w = form.write();
                            w.launch_kind = k;
                            if *k == "factorio" && w.launch_path.trim().is_empty() {
                                w.launch_path = "bin/x64/factorio".into();
                            }
                        }
                    },
                    for (k, label) in launch_kinds.iter() {
                        option { key: "{k}", value: "{k}", selected: *k == f.launch_kind, "{label}" }
                    }
                }
            }
            SettingsRow { label: path_label, hint: "Pfad relativ zum Server-Ordner",
                input {
                    class: "{INPUT} {TEXT_W} font-mono",
                    value: "{f.launch_path}",
                    placeholder: path_hint,
                    oninput: move |e| form.write().launch_path = e.value(),
                }
            }
            if uses_java {
                SettingsRow {
                    label: "Java",
                    hint: match &java_now {
                        Some(j) => format!("Aktuell verwendet: {j}"),
                        None => "Noch keine passende Java-Version gefunden – wird beim Start geladen.".to_string(),
                    },
                    select {
                        class: "{SELECT} {TEXT_W}",
                        onchange: move |e| form.write().java = e.value(),
                        option { value: "", selected: f.java.is_empty(), "Automatisch (passend zur Version)" }
                        for j in javas.iter() {
                            option {
                                key: "{j.path}",
                                value: "{j.path}",
                                selected: j.path == f.java,
                                "Java {j.major} ({j.version}) – {j.path}"
                            }
                        }
                        if !f.java.is_empty() && !javas.iter().any(|j| j.path == f.java) {
                            option { value: "{f.java}", selected: true, "{f.java}" }
                        }
                    }
                }
                SettingsRow {
                    label: "Arbeitsspeicher",
                    hint: if total_mb > 0 { format!("Host: {} frei von {}", fmt_gb(free_mb), fmt_gb(total_mb)) } else { String::new() },
                    div { class: "flex items-center gap-3",
                        input {
                            class: "w-40 accent-primary sm:w-56",
                            r#type: "range",
                            min: "512",
                            max: "{max_mb}",
                            step: "256",
                            value: "{f.memory_mb}",
                            oninput: move |e| {
                                if let Ok(v) = e.value().parse::<u32>() {
                                    form.write().memory_mb = v;
                                }
                            },
                        }
                        span {
                            class: if total_mb > 0 && f.memory_mb > free_mb { "w-16 text-right text-sm tabular-nums text-warning" } else { "w-16 text-right text-sm tabular-nums" },
                            "{fmt_gb(f.memory_mb)}"
                        }
                    }
                }
                div { class: "flex flex-col gap-2 px-3 py-3",
                    div { class: "flex flex-wrap items-center justify-between gap-2",
                        div {
                            p { class: "text-sm", "JVM-Argumente" }
                            p { class: "text-xs text-muted-foreground", "Ohne -Xmx/-Xms – der Arbeitsspeicher wird oben eingestellt." }
                        }
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            disabled: aikar_active,
                            onclick: move |_| {
                                let mut w = form.write();
                                let cur = w.jvm_args.trim().to_string();
                                w.jvm_args = if cur.is_empty() { AIKAR_FLAGS.to_string() } else { format!("{cur} {AIKAR_FLAGS}") };
                            },
                            icons::WandSparkles { class: "size-4" }
                            if aikar_active { "Aikar-Flags aktiv" } else { "Aikar-Flags einfügen" }
                        }
                    }
                    textarea {
                        class: "{TEXTAREA} min-h-20 font-mono text-xs",
                        value: "{f.jvm_args}",
                        placeholder: "z. B. -XX:+UseG1GC",
                        oninput: move |e| form.write().jvm_args = e.value(),
                    }
                }
            }
            SettingsRow { label: "Zusätzliche Argumente", hint: "Werden ans Ende des Startbefehls gehängt.",
                input {
                    class: "{INPUT} {TEXT_W} font-mono",
                    value: "{f.extra_args}",
                    oninput: move |e| form.write().extra_args = e.value(),
                }
            }
            div { class: "flex flex-col gap-1.5 px-3 py-3",
                p { class: "text-xs font-medium uppercase tracking-wider text-muted-foreground", "Vorschau" }
                code { class: "block overflow-x-auto rounded-md bg-muted/50 px-3 py-2 font-mono text-xs break-all whitespace-pre-wrap", "{preview}" }
            }
        }

        if let Some(e) = error() {
            ErrorBox { message: e }
        }
        if dirty {
            div { class: "sticky bottom-4 z-10 flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-popover px-4 py-3 shadow-lg animate-nrz-enter",
                span { class: "text-sm", "Ungespeicherte Änderungen" }
                div { class: "flex gap-2",
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        onclick: move |_| {
                            form.set(orig());
                            error.set(None);
                        },
                        "Verwerfen"
                    }
                    Button { size: ButtonSize::Sm, disabled: saving(), onclick: save,
                        if saving() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::Save { class: "size-4" }
                        }
                        "Speichern"
                    }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                                RCON                                */
/* ------------------------------------------------------------------ */

#[component]
fn RconGroup(summary: ServerSummary) -> Element {
    let ctx = use_server();
    let mut busy = use_signal(|| false);
    let id = summary.config.id.clone();
    let rcon = summary.config.rcon.clone();
    let has = rcon.is_some();

    let setup = move |_| {
        let id = id.clone();
        let mut summary_sig = ctx.summary;
        busy.set(true);
        spawn(async move {
            match api::post_empty::<ServerSummary>(&routes::server(&id, "rcon/enable")).await {
                Ok(s) => {
                    summary_sig.set(Some(s));
                    toast("RCON eingerichtet – aktiv nach dem nächsten Start");
                    refresh_servers().await;
                }
                Err(e) => toast_err(e),
            }
            busy.set(false);
        });
    };

    rsx! {
        SettingsGroup {
            title: "RCON",
            footer: "Über RCON liest das Panel Spielerliste, Positionen und Inventare live aus. Das Passwort wird zufällig erzeugt.",
            SettingsRow { label: "Status",
                if has {
                    Tag { tone: "success", "Eingerichtet" }
                } else {
                    Tag { "Nicht eingerichtet" }
                }
                Button {
                    size: ButtonSize::Sm,
                    variant: if has { ButtonVariant::Outline } else { ButtonVariant::Default },
                    disabled: busy(),
                    onclick: setup,
                    icons::KeyRound { class: "size-4" }
                    if has { "Neu einrichten" } else { "Einrichten" }
                }
            }
            if let Some(r) = rcon {
                SettingsRow { label: "Port",
                    span { class: "font-mono text-sm tabular-nums", "{r.port}" }
                }
                SettingsRow { label: "Passwort",
                    CopyField { class: "w-full sm:w-72", value: r.password.clone(), secret: true }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                         server.properties                          */
/* ------------------------------------------------------------------ */

#[derive(PartialEq)]
enum Kind {
    Text,
    Num,
    Bool,
    Choice(&'static [(&'static str, &'static str)]),
}

struct Known {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    kind: Kind,
    default: &'static str,
}

const DIFFICULTY: &[(&str, &str)] = &[("peaceful", "Friedlich"), ("easy", "Einfach"), ("normal", "Normal"), ("hard", "Schwer")];
const GAMEMODE: &[(&str, &str)] = &[("survival", "Überleben"), ("creative", "Kreativ"), ("adventure", "Abenteuer"), ("spectator", "Zuschauer")];

const K_GENERAL: &[Known] = &[
    Known { key: "motd", label: "Beschreibung (MOTD)", hint: "Text in der Serverliste", kind: Kind::Text, default: "A Minecraft Server" },
    Known { key: "max-players", label: "Max. Spieler", hint: "", kind: Kind::Num, default: "20" },
    Known { key: "online-mode", label: "Online-Modus", hint: "Nur Spieler mit gekauftem Konto (empfohlen)", kind: Kind::Bool, default: "true" },
    Known { key: "white-list", label: "Whitelist", hint: "Nur freigeschaltete Spieler dürfen beitreten", kind: Kind::Bool, default: "false" },
    Known { key: "enforce-whitelist", label: "Whitelist durchsetzen", hint: "Nicht freigeschaltete Spieler werden beim Aktivieren gekickt", kind: Kind::Bool, default: "false" },
    Known { key: "pvp", label: "PvP", hint: "Spieler können sich gegenseitig verletzen", kind: Kind::Bool, default: "true" },
    Known { key: "difficulty", label: "Schwierigkeit", hint: "", kind: Kind::Choice(DIFFICULTY), default: "easy" },
    Known { key: "gamemode", label: "Spielmodus", hint: "Für neue Spieler", kind: Kind::Choice(GAMEMODE), default: "survival" },
    Known { key: "force-gamemode", label: "Spielmodus erzwingen", hint: "Beim Beitreten immer auf den Spielmodus setzen", kind: Kind::Bool, default: "false" },
    Known { key: "hardcore", label: "Hardcore", hint: "Nach dem Tod nur noch Zuschauer", kind: Kind::Bool, default: "false" },
    Known { key: "allow-flight", label: "Fliegen erlauben", hint: "Verhindert Kicks durch Fly-Mods", kind: Kind::Bool, default: "false" },
    Known { key: "enable-command-block", label: "Befehlsblöcke", hint: "", kind: Kind::Bool, default: "false" },
    Known { key: "player-idle-timeout", label: "AFK-Kick (Minuten)", hint: "0 = nie", kind: Kind::Num, default: "0" },
];

const K_WORLD: &[Known] = &[
    Known { key: "level-name", label: "Weltordner", hint: "Ordnername der Welt", kind: Kind::Text, default: "world" },
    Known { key: "level-seed", label: "Seed", hint: "Nur für neu erzeugte Welten", kind: Kind::Text, default: "" },
    Known { key: "view-distance", label: "Sichtweite (Chunks)", hint: "Höher = mehr Last", kind: Kind::Num, default: "10" },
    Known { key: "simulation-distance", label: "Simulationsweite (Chunks)", hint: "", kind: Kind::Num, default: "10" },
    Known { key: "spawn-protection", label: "Spawn-Schutz (Blöcke)", hint: "0 = aus", kind: Kind::Num, default: "16" },
    Known { key: "allow-nether", label: "Nether erlauben", hint: "", kind: Kind::Bool, default: "true" },
    Known { key: "spawn-monsters", label: "Monster", hint: "", kind: Kind::Bool, default: "true" },
    Known { key: "spawn-animals", label: "Tiere", hint: "", kind: Kind::Bool, default: "true" },
    Known { key: "spawn-npcs", label: "Dorfbewohner", hint: "", kind: Kind::Bool, default: "true" },
    Known { key: "generate-structures", label: "Strukturen erzeugen", hint: "Dörfer, Tempel …", kind: Kind::Bool, default: "true" },
];

/// Vom Panel verwaltet → in „Weitere“ nur lesbar.
const MANAGED: &[&str] = &["server-port", "enable-rcon", "rcon.port", "rcon.password"];

fn is_known(k: &str) -> bool {
    K_GENERAL.iter().chain(K_WORLD.iter()).any(|x| x.key == k)
}

fn get_prop(list: &[(String, String)], k: &str) -> Option<String> {
    list.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
}

fn set_prop(mut sig: Signal<Option<Vec<(String, String)>>>, k: &str, v: String) {
    let mut w = sig.write();
    let Some(list) = w.as_mut() else { return };
    if let Some(e) = list.iter_mut().find(|(key, _)| key == k) {
        e.1 = v;
    } else {
        list.push((k.to_string(), v));
    }
}

fn prop_row(known: &'static Known, value: String, entries: Signal<Option<Vec<(String, String)>>>) -> Element {
    let key = known.key;
    let hint = if known.hint.is_empty() { None } else { Some(known.hint.to_string()) };
    let control = match &known.kind {
        Kind::Bool => rsx! {
            Toggle {
                checked: value == "true",
                label: known.label,
                onchange: move |v: bool| set_prop(entries, key, v.to_string()),
            }
        },
        Kind::Num => rsx! {
            input {
                class: "{INPUT} w-28",
                r#type: "number",
                value: "{value}",
                oninput: move |e| set_prop(entries, key, e.value()),
            }
        },
        Kind::Text => rsx! {
            input {
                class: "{INPUT} {TEXT_W}",
                value: "{value}",
                oninput: move |e| set_prop(entries, key, e.value()),
            }
        },
        Kind::Choice(opts) => {
            let opts: &'static [(&'static str, &'static str)] = opts;
            let custom = !opts.iter().any(|(v, _)| *v == value);
            rsx! {
                select {
                    class: "{SELECT} w-40",
                    onchange: move |e| set_prop(entries, key, e.value()),
                    for (v, l) in opts.iter() {
                        option { key: "{v}", value: "{v}", selected: *v == value, "{l}" }
                    }
                    if custom {
                        option { value: "{value}", selected: true, "{value}" }
                    }
                }
            }
        }
    };
    rsx! {
        SettingsRow { key: "{key}", label: known.label, hint, {control} }
    }
}

#[component]
fn PropertiesEditor(id: String) -> Element {
    let ctx = use_server();
    let entries = use_signal(|| None::<Vec<(String, String)>>);
    let mut orig = use_signal(Vec::<(String, String)>::new);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);
    let mut restart_hint = use_signal(|| false);
    let mut show_more = use_signal(|| false);
    let mut filter = use_signal(String::new);

    let load_id = id.clone();
    use_future(move || {
        let id = load_id.clone();
        let mut entries = entries;
        async move {
            match api::get::<Properties>(&routes::server(&id, "mc/properties")).await {
                Ok(p) => {
                    orig.set(p.entries.clone());
                    entries.set(Some(p.entries));
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });

    let Some(list) = entries() else {
        return rsx! {
            SettingsGroup { title: "server.properties",
                if let Some(e) = error() {
                    div { class: "p-3", ErrorBox { message: e } }
                } else {
                    Loading {}
                }
            }
        };
    };

    let dirty = list != orig();
    let online = ctx.status.read().is_online();
    let q = filter().to_lowercase();
    let others: Vec<(String, String)> = list
        .iter()
        .filter(|(k, _)| !is_known(k))
        .filter(|(k, v)| q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let other_count = list.iter().filter(|(k, _)| !is_known(k)).count();

    let save_id = id.clone();
    let save = move |_| {
        let Some(list) = entries() else { return };
        let id = save_id.clone();
        saving.set(true);
        error.set(None);
        spawn(async move {
            match api::put::<_, nrz_gp_common::Ok>(&routes::server(&id, "mc/properties"), &Properties { entries: list.clone() }).await {
                Ok(_) => {
                    orig.set(list);
                    toast("server.properties gespeichert");
                    if ctx.status.peek().is_online() {
                        restart_hint.set(true);
                    }
                }
                Err(e) => error.set(Some(e)),
            }
            saving.set(false);
        });
    };
    let restart_id = id.clone();

    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "flex flex-wrap items-end justify-between gap-3 px-1",
                div {
                    h2 { class: "font-semibold", "Spieleinstellungen" }
                    p { class: "text-sm text-muted-foreground", "Inhalt der server.properties. Änderungen wirken nach einem Neustart." }
                }
                div { class: "flex gap-2",
                    if dirty {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            onclick: move |_| {
                                let mut e = entries;
                                e.set(Some(orig()));
                            },
                            "Verwerfen"
                        }
                    }
                    Button { size: ButtonSize::Sm, disabled: !dirty || saving(), onclick: save,
                        if saving() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        } else {
                            icons::Save { class: "size-4" }
                        }
                        "Speichern"
                    }
                }
            }
            if let Some(e) = error() {
                ErrorBox { message: e }
            }
            if restart_hint() && online {
                Notice {
                    warning: true,
                    action: rsx! {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Outline,
                            onclick: move |_| {
                                let id = restart_id.clone();
                                restart_hint.set(false);
                                spawn(async move { power(&id, PowerAction::Restart).await });
                            },
                            icons::RotateCw { class: "size-4" }
                            "Jetzt neu starten"
                        }
                    },
                    "Gespeichert – die Änderungen werden erst nach einem Neustart wirksam."
                }
            }
            SettingsGroup { title: "Spiel",
                for k in K_GENERAL.iter() {
                    {prop_row(k, get_prop(&list, k.key).unwrap_or_else(|| k.default.to_string()), entries)}
                }
            }
            SettingsGroup { title: "Welt",
                for k in K_WORLD.iter() {
                    {prop_row(k, get_prop(&list, k.key).unwrap_or_else(|| k.default.to_string()), entries)}
                }
            }
            div { class: "grid gap-2",
                button {
                    r#type: "button",
                    class: "flex items-center gap-1.5 px-1 text-left text-sm font-semibold",
                    onclick: move |_| show_more.toggle(),
                    if show_more() {
                        icons::ChevronDown { class: "size-4" }
                    } else {
                        icons::ChevronRight { class: "size-4" }
                    }
                    "Weitere ({other_count})"
                }
                if show_more() {
                    input {
                        class: INPUT,
                        placeholder: "Schlüssel oder Wert suchen …",
                        value: "{filter}",
                        oninput: move |e| filter.set(e.value()),
                    }
                    div { class: "divide-y rounded-lg border bg-card",
                        if others.is_empty() {
                            p { class: "px-3 py-6 text-center text-sm text-muted-foreground", "Keine weiteren Einträge" }
                        }
                        for (k, v) in others {
                            {
                                let managed = MANAGED.contains(&k.as_str());
                                let key = k.clone();
                                rsx! {
                                    div { key: "{k}", class: "flex flex-wrap items-center justify-between gap-x-4 gap-y-1.5 px-3 py-2",
                                        div { class: "min-w-0",
                                            p { class: "font-mono text-xs break-all", "{k}" }
                                            if managed {
                                                p { class: "text-xs text-muted-foreground", "Wird vom Panel verwaltet" }
                                            }
                                        }
                                        input {
                                            class: "{INPUT} {TEXT_W} font-mono text-xs",
                                            value: "{v}",
                                            disabled: managed,
                                            oninput: move |e| set_prop(entries, &key, e.value()),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/*                            Gefahrenzone                            */
/* ------------------------------------------------------------------ */

#[component]
fn DangerZone(summary: ServerSummary) -> Element {
    let mut open = use_signal(|| false);
    let mut typed = use_signal(String::new);
    let mut delete_files = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let nav = navigator();
    let name = summary.config.name.clone();
    let imported = summary.config.imported;
    let dir = summary.config.dir.clone();
    let id = summary.config.id.clone();
    let matches = typed().trim() == name.trim();

    let do_delete = use_callback(move |_: ()| {
        if busy() {
            return;
        }
        busy.set(true);
        let files = delete_files() && !imported;
        let url = format!("{}{}", routes::server(&id, ""), if files { "?delete_files=true" } else { "" });
        spawn(async move {
            match api::delete::<nrz_gp_common::Ok>(&url).await {
                Ok(_) => {
                    toast("Server gelöscht");
                    refresh_servers().await;
                    nav.push(Route::Dashboard {});
                }
                Err(e) => {
                    toast_err(e);
                    busy.set(false);
                }
            }
        });
    });

    rsx! {
        div { class: "grid gap-2",
            h3 { class: "px-1 text-sm font-semibold text-destructive", "Gefahrenzone" }
            div { class: "rounded-lg border border-destructive/30 bg-card",
                div { class: "flex flex-wrap items-center justify-between gap-3 px-3 py-3",
                    div { class: "min-w-0",
                        p { class: "text-sm", "Server löschen" }
                        p { class: "text-xs text-muted-foreground",
                            if imported {
                                "Entfernt den Server aus dem Panel. Der importierte Ordner bleibt unverändert erhalten."
                            } else {
                                "Entfernt den Server aus dem Panel, auf Wunsch samt allen Dateien."
                            }
                        }
                    }
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Destructive,
                        onclick: move |_| {
                            typed.set(String::new());
                            delete_files.set(false);
                            open.set(true);
                        },
                        icons::Trash2 { class: "size-4" }
                        "Server löschen"
                    }
                }
            }
        }
        if open() {
            Modal {
                title: "Server löschen?",
                description: "Diese Aktion kann nicht rückgängig gemacht werden.",
                onclose: move |_| open.set(false),
                footer: rsx! {
                    Button { variant: ButtonVariant::Outline, onclick: move |_| open.set(false), "Abbrechen" }
                    Button {
                        variant: ButtonVariant::Destructive,
                        disabled: !matches || busy(),
                        onclick: move |_| do_delete.call(()),
                        if busy() {
                            icons::LoaderCircle { class: "size-4 animate-spin" }
                        }
                        "Endgültig löschen"
                    }
                },
                div { class: "flex flex-col gap-4",
                    p { class: "text-sm text-muted-foreground",
                        "Gib zur Bestätigung den Namen "
                        span { class: "font-medium text-foreground", "„{name}“" }
                        " ein."
                    }
                    input {
                        class: INPUT,
                        autofocus: true,
                        value: "{typed}",
                        placeholder: "{name}",
                        oninput: move |e| typed.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Enter && matches {
                                do_delete.call(());
                            }
                        },
                    }
                    if imported {
                        Notice { "Importierter Server: der Ordner {dir} wird nicht gelöscht." }
                    } else {
                        CheckRow {
                            checked: delete_files(),
                            onchange: move |v| delete_files.set(v),
                            label: "Auch alle Dateien löschen",
                            hint: format!("Welten, Plugins/Mods und Einstellungen in {dir} werden unwiderruflich entfernt."),
                        }
                    }
                }
            }
        }
    }
}
