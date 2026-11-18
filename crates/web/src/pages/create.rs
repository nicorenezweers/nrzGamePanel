//! Server erstellen – Assistent in Schritten: Spiel → Software → Version → Einstellungen → Zusammenfassung.

use dioxus::prelude::*;
use nrz_gp_common::{CreateServer as CreateRequest, Flavor, Game, PortCheck, ServerSummary, VersionList, routes::enc};

use crate::{
    Route, api,
    components::{
        CARD, CheckRow, ErrorBox, Field, INPUT, Loading, Notice, SELECT, Tag, fmt_bytes,
        ui::{
            app_shell::{PageContainer, PageHeader, PageWidth},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    store::{SYSTEM, refresh_servers, toast},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Game,
    Software,
    Version,
    Settings,
    Summary,
}

impl Step {
    const fn label(self) -> &'static str {
        match self {
            Self::Game => "Spiel",
            Self::Software => "Software",
            Self::Version => "Version",
            Self::Settings => "Einstellungen",
            Self::Summary => "Zusammenfassung",
        }
    }
}

fn steps(game: Game) -> Vec<Step> {
    match game {
        Game::Minecraft => vec![Step::Game, Step::Software, Step::Version, Step::Settings, Step::Summary],
        Game::Factorio => vec![Step::Game, Step::Version, Step::Settings, Step::Summary],
    }
}

/// serde-Name eines Enums (z. B. `Flavor::NeoForge` → `neo_forge`) für URL-Pfade.
fn slug<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn flavor_info(f: Flavor) -> (&'static str, &'static str) {
    match f {
        Flavor::Paper => ("Schnell und stabil – die beste Wahl für die meisten Server.", "Plugins (Bukkit, Spigot, Paper)"),
        Flavor::Vanilla => ("Das Original von Mojang, ganz ohne Änderungen.", "Keine Plugins oder Mods"),
        Flavor::Fabric => ("Leichter Mod-Loader, ideal für Performance- und Technik-Mods.", "Mods (Fabric)"),
        Flavor::Purpur => ("Paper mit vielen zusätzlichen Einstellungen für Spielmechaniken.", "Plugins (Bukkit, Spigot, Paper)"),
        Flavor::NeoForge => ("Moderner Forge-Nachfolger, für aktuelle Modpacks.", "Mods (NeoForge)"),
        Flavor::Forge => ("Der Klassiker unter den Mod-Loadern mit riesiger Auswahl.", "Mods (Forge)"),
        _ => ("", ""),
    }
}

const fn recommended_mb(f: Flavor) -> u32 {
    match f {
        Flavor::Paper | Flavor::Purpur | Flavor::Fabric => 4096,
        Flavor::Forge | Flavor::NeoForge => 6144,
        _ => 2048,
    }
}

const fn needs_loader(f: Flavor) -> bool {
    matches!(f, Flavor::Fabric | Flavor::Forge | Flavor::NeoForge)
}

fn gb(mb: u32) -> String {
    let g = f64::from(mb) / 1024.0;
    if (g - g.round()).abs() < 0.01 { format!("{g:.0} GB") } else { format!("{g:.1} GB") }
}

fn difficulty_label(d: &str) -> &'static str {
    match d {
        "peaceful" => "Friedlich",
        "normal" => "Normal",
        "hard" => "Schwer",
        _ => "Einfach",
    }
}

fn gamemode_label(g: &str) -> &'static str {
    match g {
        "creative" => "Kreativ",
        "adventure" => "Abenteuer",
        "spectator" => "Zuschauer",
        _ => "Überleben",
    }
}

fn non_empty(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[component]
pub fn CreateServer() -> Element {
    let nav = navigator();
    let mut step = use_signal(|| Step::Game);
    let mut game = use_signal(|| Game::Minecraft);
    let mut flavor = use_signal(|| Flavor::Paper);
    let mut version = use_signal(|| None::<String>);
    let mut loader = use_signal(|| None::<String>);
    let mut search = use_signal(String::new);
    let mut stable_only = use_signal(|| true);
    let mut name = use_signal(String::new);
    let mut memory = use_signal(|| recommended_mb(Flavor::Paper));
    let mut port = use_signal(|| Game::Minecraft.default_port().to_string());
    let mut advanced = use_signal(|| false);
    let mut motd = use_signal(String::new);
    let mut max_players = use_signal(|| "20".to_string());
    let mut online_mode = use_signal(|| true);
    let mut difficulty = use_signal(|| "easy".to_string());
    let mut gamemode = use_signal(|| "survival".to_string());
    let mut seed = use_signal(String::new);
    let mut auto_start = use_signal(|| true);
    let mut eula = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut finding = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // Versionen der gewählten Software
    let versions = use_resource(move || async move {
        let (g, f) = (game(), flavor());
        api::get::<VersionList>(&format!("/api/versions/{}/{}", slug(&g), slug(&f))).await
    });
    // Neueste stabile Version vorauswählen
    use_effect(move || {
        if let Some(Ok(list)) = &*versions.read() {
            let cur = version.peek().clone();
            if cur.as_ref().is_none_or(|c| !list.versions.iter().any(|v| &v.id == c)) {
                let pick = list.versions.iter().find(|v| v.stable).or(list.versions.first()).map(|v| v.id.clone());
                version.set(pick);
            }
        }
    });
    // Loader-Versionen (Fabric/Forge/NeoForge) zur gewählten Spielversion
    let loaders = use_resource(move || async move {
        let (g, f, v) = (game(), flavor(), version());
        match v {
            Some(v) if needs_loader(f) => api::get::<VersionList>(&format!("/api/versions/{}/{}?game_version={}", slug(&g), slug(&f), enc(&v)))
                .await
                .map(|l| l.loader_versions),
            _ => Ok(Vec::new()),
        }
    });
    use_effect(move || {
        if let Some(Ok(list)) = &*loaders.read() {
            let cur = loader.peek().clone();
            if cur.as_ref().is_none_or(|c| !list.contains(c)) {
                loader.set(list.first().cloned());
            }
        }
    });
    // Port live prüfen (kurz entprellt)
    let port_check = use_resource(move || async move {
        let (p, g) = (port(), game());
        let n = p.trim().parse::<u16>().ok().filter(|n| *n > 0)?;
        gloo_timers::future::TimeoutFuture::new(300).await;
        api::get::<PortCheck>(&format!("/api/check-port?port={n}&game={}", slug(&g))).await.ok()
    });

    let g = game();
    let f = flavor();
    let order = steps(g);
    let cur = step();
    let idx = order.iter().position(|s| *s == cur).unwrap_or(0);
    let port_num = port().trim().parse::<u16>().ok().filter(|p| *p > 0);
    let port_state = port_check.read().clone().flatten();
    let port_taken = port_state.as_ref().is_some_and(|c| !c.free && Some(c.port) == port_num);
    let versions_failed = matches!(&*versions.read(), Some(Err(_)));
    let can_next = match cur {
        Step::Game | Step::Software => true,
        Step::Version => version().is_some() || versions_failed,
        Step::Settings => !name().trim().is_empty() && port_num.is_some() && !port_taken && (g == Game::Factorio || eula()),
        Step::Summary => !busy(),
    };
    let sys = SYSTEM.read().clone();
    let free_ram = sys.as_ref().map(|s| s.memory_total.saturating_sub(s.memory_used));
    let ram_too_much = free_ram.is_some_and(|fr| u64::from(memory()) * 1024 * 1024 > fr);

    let mut go_next = move || {
        let order = steps(game());
        if let Some(i) = order.iter().position(|s| *s == step()) {
            if let Some(n) = order.get(i + 1) {
                step.set(*n);
            }
        }
    };
    let mut go_back = move || {
        let order = steps(game());
        if let Some(i) = order.iter().position(|s| *s == step()) {
            if i > 0 {
                step.set(order[i - 1]);
            }
        }
    };
    let mut choose_game = move |new: Game| {
        if game() != new {
            game.set(new);
            let fl = if new == Game::Factorio { Flavor::Factorio } else { Flavor::Paper };
            flavor.set(fl);
            version.set(None);
            loader.set(None);
            memory.set(recommended_mb(fl));
            port.set(new.default_port().to_string());
            search.set(String::new());
        }
        let n = name();
        if n.trim().is_empty() || n == "Minecraft-Server" || n == "Factorio-Server" {
            name.set(format!("{}-Server", new.label()));
        }
        go_next();
    };
    let mut choose_flavor = move |fl: Flavor| {
        if flavor() != fl {
            flavor.set(fl);
            version.set(None);
            loader.set(None);
            memory.set(recommended_mb(fl));
        }
        go_next();
    };
    let find_free = move |_| {
        let start = port().trim().parse::<u16>().unwrap_or_else(|_| game().default_port());
        finding.set(true);
        spawn(async move {
            let gs = slug(&game());
            let mut found = false;
            for p in start.saturating_add(1)..start.saturating_add(200) {
                if let Ok(c) = api::get::<PortCheck>(&format!("/api/check-port?port={p}&game={gs}")).await {
                    if c.free {
                        port.set(p.to_string());
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                crate::store::toast_err("Kein freier Port in der Nähe gefunden");
            }
            finding.set(false);
        });
    };
    let mut create = move || {
        let g = game();
        let fl = flavor();
        let Some(p) = port().trim().parse::<u16>().ok() else { return };
        busy.set(true);
        error.set(None);
        let mc = g == Game::Minecraft;
        let req = CreateRequest {
            name: name().trim().to_string(),
            game: g,
            flavor: fl,
            version: version(),
            loader_version: if needs_loader(fl) { loader() } else { None },
            memory_mb: memory(),
            port: p,
            accept_eula: mc && eula(),
            motd: non_empty(motd()),
            max_players: max_players().trim().parse().ok(),
            online_mode: mc.then(|| online_mode()),
            difficulty: mc.then(|| difficulty()),
            gamemode: mc.then(|| gamemode()),
            seed: if mc { non_empty(seed()) } else { None },
            auto_start: auto_start(),
        };
        spawn(async move {
            match api::post::<_, ServerSummary>("/api/servers", &req).await {
                Ok(s) => {
                    refresh_servers().await;
                    toast("Server angelegt – Installation läuft");
                    nav.push(Route::ServerOverview { id: s.config.id });
                }
                Err(e) => {
                    error.set(Some(e));
                    busy.set(false);
                }
            }
        });
    };

    let (title, subtitle) = match cur {
        Step::Game => ("Welches Spiel?", "Wähle, welchen Server du erstellen möchtest."),
        Step::Software => ("Welche Server-Software?", "Bestimmt, ob du Plugins oder Mods nutzen kannst. Im Zweifel: Paper."),
        Step::Version => ("Welche Version?", "Die neueste stabile Version ist vorausgewählt."),
        Step::Settings => ("Einstellungen", "Name, Arbeitsspeicher und Port. Alles lässt sich später ändern."),
        Step::Summary => ("Alles bereit?", "Prüfe die Angaben und erstelle den Server."),
    };

    rsx! {
        PageContainer { width: PageWidth::Narrow,
            PageHeader {
                title: "Server erstellen",
                description: "Das Panel lädt die Server-Software herunter und richtet alles ein.",
            }

            // Fortschritt
            ol { class: "mb-6 flex items-center gap-2",
                for (i , s) in order.iter().copied().enumerate() {
                    {
                        let done = i < idx;
                        let active = i == idx;
                        let last = i + 1 == order.len();
                        rsx! {
                            li { key: "{s:?}", class: if last { "flex items-center" } else { "flex flex-1 items-center gap-2" },
                                button {
                                    r#type: "button",
                                    class: "flex items-center gap-2 rounded-md text-sm disabled:cursor-default",
                                    disabled: !done,
                                    onclick: move |_| step.set(s),
                                    span {
                                        class: if active {
                                            "flex size-7 shrink-0 items-center justify-center rounded-full bg-primary text-xs font-semibold text-primary-foreground"
                                        } else if done {
                                            "flex size-7 shrink-0 items-center justify-center rounded-full bg-muted text-foreground hover:bg-accent"
                                        } else {
                                            "flex size-7 shrink-0 items-center justify-center rounded-full border text-xs text-muted-foreground"
                                        },
                                        if done {
                                            icons::Check { class: "size-3.5" }
                                        } else {
                                            "{i + 1}"
                                        }
                                    }
                                    span {
                                        class: if active { "font-medium" } else { "hidden text-muted-foreground md:inline" },
                                        "{s.label()}"
                                    }
                                }
                                if !last {
                                    div { class: if done { "h-px min-w-3 flex-1 bg-foreground/40" } else { "h-px min-w-3 flex-1 bg-border" } }
                                }
                            }
                        }
                    }
                }
            }

            form {
                class: "flex flex-col gap-5",
                onsubmit: move |e: FormEvent| {
                    e.prevent_default();
                    if !can_next {
                        return;
                    }
                    if step() == Step::Summary {
                        create();
                    } else {
                        go_next();
                    }
                },
                div {
                    h2 { class: "text-lg font-semibold", "{title}" }
                    p { class: "text-sm text-muted-foreground", "{subtitle}" }
                }

                match cur {
                    Step::Game => rsx! {
                        div { class: "grid gap-3 sm:grid-cols-2",
                            GameCard {
                                selected: g == Game::Minecraft,
                                title: "Minecraft",
                                text: "Java Edition – Vanilla, Paper, Fabric, Forge und mehr.",
                                onclick: move |_| choose_game(Game::Minecraft),
                                icon: rsx! { icons::Pickaxe { class: "size-6" } },
                            }
                            GameCard {
                                selected: g == Game::Factorio,
                                title: "Factorio",
                                text: "Headless-Server für gemeinsame Fabriken.",
                                tag: "nur Linux x86_64",
                                onclick: move |_| choose_game(Game::Factorio),
                                icon: rsx! { icons::Factory { class: "size-6" } },
                            }
                        }
                    },
                    Step::Software => rsx! {
                        div { class: "grid gap-3 sm:grid-cols-2",
                            for fl in Flavor::installable().iter().copied() {
                                {
                                    let (desc, addons) = flavor_info(fl);
                                    let sel = fl == f;
                                    rsx! {
                                        button {
                                            key: "{fl.label()}",
                                            r#type: "button",
                                            class: if sel {
                                                "flex flex-col gap-1.5 rounded-xl border border-foreground/40 bg-accent/40 p-4 text-left shadow-sm ring-1 ring-foreground/20"
                                            } else {
                                                "flex flex-col gap-1.5 rounded-xl border bg-card p-4 text-left shadow-sm transition-colors hover:bg-accent/40"
                                            },
                                            onclick: move |_| choose_flavor(fl),
                                            div { class: "flex items-center gap-2",
                                                span { class: "font-medium", "{fl.label()}" }
                                                if fl == Flavor::Paper {
                                                    Tag { tone: "success", "Empfohlen" }
                                                }
                                                if sel {
                                                    icons::CircleCheck { class: "ml-auto size-4" }
                                                }
                                            }
                                            p { class: "text-sm text-muted-foreground", "{desc}" }
                                            p { class: "mt-auto flex items-center gap-1.5 pt-1 text-xs text-muted-foreground",
                                                icons::Puzzle { class: "size-3.5" }
                                                "{addons}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Step::Version => rsx! {
                        if g == Game::Factorio {
                            Notice { warning: true,
                                "Der Factorio-Server läuft nur auf Linux x86_64 (nicht auf ARM, z. B. Raspberry Pi)."
                            }
                        }
                        match &*versions.read() {
                            None => rsx! { Loading { text: "Lade Versionen …" } },
                            Some(Err(e)) => rsx! {
                                ErrorBox { message: format!("Versionen konnten nicht geladen werden: {e}") }
                                div { class: "flex flex-wrap items-center gap-2 text-sm text-muted-foreground",
                                    Button {
                                        size: ButtonSize::Sm,
                                        variant: ButtonVariant::Outline,
                                        onclick: move |_| versions.clone().restart(),
                                        icons::RefreshCw { class: "size-4" }
                                        "Erneut versuchen"
                                    }
                                    span { "oder weiter mit der neuesten stabilen Version." }
                                }
                            },
                            Some(Ok(list)) => {
                                let q = search().trim().to_lowercase();
                                let newest_stable = list.versions.iter().find(|v| v.stable).map(|v| v.id.clone());
                                let newest = list.versions.first().map(|v| v.id.clone());
                                let shown: Vec<_> = list
                                    .versions
                                    .iter()
                                    .filter(|v| !stable_only() || v.stable || version().as_deref() == Some(v.id.as_str()))
                                    .filter(|v| q.is_empty() || v.id.to_lowercase().contains(&q))
                                    .take(240)
                                    .cloned()
                                    .collect();
                                rsx! {
                                    if g == Game::Factorio {
                                        div { class: "grid gap-3 sm:grid-cols-2",
                                            if let Some(v) = newest_stable.clone() {
                                                GameCard {
                                                    selected: version().as_deref() == Some(v.as_str()),
                                                    title: "Stabil",
                                                    text: format!("Version {v} – empfohlen"),
                                                    onclick: move |_| version.set(Some(v.clone())),
                                                    icon: rsx! { icons::ShieldCheck { class: "size-6" } },
                                                }
                                            }
                                            if let Some(v) = newest.clone().filter(|n| Some(n) != newest_stable.as_ref()) {
                                                GameCard {
                                                    selected: version().as_deref() == Some(v.as_str()),
                                                    title: "Experimentell",
                                                    text: format!("Version {v} – neueste Funktionen, evtl. Fehler"),
                                                    onclick: move |_| version.set(Some(v.clone())),
                                                    icon: rsx! { icons::FlaskConical { class: "size-6" } },
                                                }
                                            }
                                        }
                                    }
                                    div { class: "{CARD} flex flex-col gap-3 p-4",
                                        div { class: "flex flex-wrap items-center gap-3",
                                            div { class: "relative min-w-48 flex-1",
                                                icons::Search { class: "pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" }
                                                input {
                                                    class: "{INPUT} pl-8",
                                                    placeholder: "Version suchen, z. B. 1.21",
                                                    value: "{search}",
                                                    oninput: move |e| search.set(e.value()),
                                                }
                                            }
                                            CheckRow { checked: stable_only(), onchange: move |v| stable_only.set(v), label: "Nur stabile" }
                                        }
                                        if shown.is_empty() {
                                            p { class: "py-6 text-center text-sm text-muted-foreground", "Keine passende Version gefunden." }
                                        }
                                        div { class: "grid max-h-72 grid-cols-3 gap-1.5 overflow-y-auto pr-1 sm:grid-cols-4",
                                            for v in shown {
                                                {
                                                    let sel = version().as_deref() == Some(v.id.as_str());
                                                    let id = v.id.clone();
                                                    let is_newest = newest_stable.as_deref() == Some(v.id.as_str());
                                                    rsx! {
                                                        button {
                                                            key: "{v.id}",
                                                            r#type: "button",
                                                            class: if sel {
                                                                "flex items-center justify-between gap-1 rounded-md bg-primary px-2.5 py-1.5 text-left font-mono text-xs text-primary-foreground"
                                                            } else {
                                                                "flex items-center justify-between gap-1 rounded-md border px-2.5 py-1.5 text-left font-mono text-xs hover:bg-accent"
                                                            },
                                                            title: if is_newest { "Neueste stabile Version" } else if !v.stable { "Vorabversion" } else { "" },
                                                            onclick: move |_| version.set(Some(id.clone())),
                                                            span { class: "truncate", "{v.id}" }
                                                            if is_newest {
                                                                icons::Star { class: "size-3 shrink-0" }
                                                            } else if !v.stable {
                                                                icons::FlaskConical { class: "size-3 shrink-0 opacity-70" }
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
                        if needs_loader(f) && version().is_some() {
                            div { class: "{CARD} p-4",
                                Field {
                                    label: format!("{}-Loader", f.label()),
                                    hint: "Die neueste Loader-Version ist vorausgewählt.",
                                    match &*loaders.read() {
                                        None => rsx! { span { class: "text-sm text-muted-foreground", "Lade Loader-Versionen …" } },
                                        Some(Err(e)) => rsx! { ErrorBox { message: format!("Loader-Versionen nicht verfügbar ({e}) – es wird die neueste verwendet.") } },
                                        Some(Ok(list)) if list.is_empty() => rsx! { span { class: "text-sm text-muted-foreground", "Für diese Version gibt es keine Loader-Auswahl – es wird die neueste verwendet." } },
                                        Some(Ok(list)) => rsx! {
                                            select {
                                                class: SELECT,
                                                onchange: move |e| loader.set(Some(e.value())),
                                                for l in list.iter() {
                                                    option { key: "{l}", value: "{l}", selected: loader().as_deref() == Some(l.as_str()), "{l}" }
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    },
                    Step::Settings => rsx! {
                        div { class: "{CARD} flex flex-col gap-5 p-4 sm:p-6",
                            Field { label: "Name", hint: "Nur für dich im Panel sichtbar.",
                                input {
                                    class: INPUT,
                                    value: "{name}",
                                    placeholder: "z. B. Survival mit Freunden",
                                    autofocus: true,
                                    oninput: move |e| name.set(e.value()),
                                }
                            }
                            if g == Game::Minecraft {
                                div { class: "flex flex-col gap-2",
                                    div { class: "flex items-baseline justify-between gap-2",
                                        span { class: "text-sm font-medium", "Arbeitsspeicher" }
                                        span { class: "font-mono text-sm tabular-nums", "{gb(memory())}" }
                                    }
                                    input {
                                        r#type: "range",
                                        class: "w-full accent-primary",
                                        min: "1024",
                                        max: "16384",
                                        step: "512",
                                        value: "{memory}",
                                        oninput: move |e| {
                                            if let Ok(v) = e.value().parse::<u32>() {
                                                memory.set(v);
                                            }
                                        },
                                    }
                                    div { class: "flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground",
                                        span {
                                            "Empfohlen für {f.label()}: {gb(recommended_mb(f))}"
                                            if memory() != recommended_mb(f) {
                                                " · "
                                                button {
                                                    r#type: "button",
                                                    class: "underline hover:text-foreground",
                                                    onclick: move |_| memory.set(recommended_mb(flavor())),
                                                    "übernehmen"
                                                }
                                            }
                                        }
                                        if let Some(fr) = free_ram {
                                            span { class: if ram_too_much { "text-warning" } else { "" }, "Frei auf dem Host: {fmt_bytes(fr)}" }
                                        }
                                    }
                                }
                            }
                            Field {
                                label: "Port",
                                hint: if g == Game::Minecraft { "Standard für Minecraft: 25565 (TCP)." } else { "Standard für Factorio: 34197 (UDP)." },
                                div { class: "flex flex-wrap items-center gap-2",
                                    input {
                                        class: "{INPUT} max-w-32 font-mono",
                                        inputmode: "numeric",
                                        value: "{port}",
                                        oninput: move |e| port.set(e.value().chars().filter(char::is_ascii_digit).take(5).collect()),
                                    }
                                    if port_num.is_none() {
                                        span { class: "text-sm text-destructive", "Ungültiger Port" }
                                    } else if let Some(c) = port_state.as_ref().filter(|c| Some(c.port) == port_num) {
                                        if c.free {
                                            span { class: "inline-flex items-center gap-1 text-sm text-success",
                                                icons::CircleCheck { class: "size-4" }
                                                "frei"
                                            }
                                        } else {
                                            span { class: "inline-flex items-center gap-1 text-sm text-destructive",
                                                icons::CircleAlert { class: "size-4" }
                                                match &c.used_by {
                                                    Some(n) => format!("belegt durch „{n}“"),
                                                    None => "belegt".to_string(),
                                                }
                                            }
                                        }
                                    } else {
                                        span { class: "inline-flex items-center gap-1 text-sm text-muted-foreground",
                                            icons::LoaderCircle { class: "size-4 animate-spin" }
                                            "prüfe …"
                                        }
                                    }
                                    Button {
                                        size: ButtonSize::Sm,
                                        variant: ButtonVariant::Outline,
                                        disabled: finding(),
                                        onclick: find_free,
                                        if finding() {
                                            icons::LoaderCircle { class: "size-4 animate-spin" }
                                        } else {
                                            icons::Search { class: "size-4" }
                                        }
                                        "Nächster freier"
                                    }
                                }
                            }

                            div { class: "rounded-lg border",
                                button {
                                    r#type: "button",
                                    class: "flex w-full items-center gap-2 px-3 py-2.5 text-left text-sm font-medium hover:bg-accent/40",
                                    aria_expanded: "{advanced}",
                                    onclick: move |_| advanced.toggle(),
                                    if advanced() {
                                        icons::ChevronDown { class: "size-4" }
                                    } else {
                                        icons::ChevronRight { class: "size-4" }
                                    }
                                    "Erweitert"
                                    span { class: "font-normal text-muted-foreground",
                                        if g == Game::Minecraft { "MOTD, Spieler, Schwierigkeit, Spielmodus, Seed" } else { "Beschreibung, Spieler" }
                                    }
                                }
                                if advanced() {
                                    div { class: "grid gap-4 border-t p-3 sm:grid-cols-2",
                                        Field {
                                            label: if g == Game::Minecraft { "MOTD" } else { "Beschreibung" },
                                            hint: if g == Game::Minecraft { "Text in der Serverliste des Spiels." } else { "Text in der Serverliste." },
                                            class: "sm:col-span-2",
                                            input { class: INPUT, value: "{motd}", placeholder: "A Minecraft Server", oninput: move |e| motd.set(e.value()) }
                                        }
                                        Field { label: "Max. Spieler",
                                            input {
                                                class: INPUT,
                                                inputmode: "numeric",
                                                value: "{max_players}",
                                                oninput: move |e| max_players.set(e.value().chars().filter(char::is_ascii_digit).take(4).collect()),
                                            }
                                        }
                                        if g == Game::Minecraft {
                                            Field { label: "Schwierigkeit",
                                                select { class: SELECT, onchange: move |e| difficulty.set(e.value()),
                                                    for d in ["peaceful", "easy", "normal", "hard"] {
                                                        option { key: "{d}", value: d, selected: difficulty() == d, "{difficulty_label(d)}" }
                                                    }
                                                }
                                            }
                                            Field { label: "Spielmodus",
                                                select { class: SELECT, onchange: move |e| gamemode.set(e.value()),
                                                    for m in ["survival", "creative", "adventure", "spectator"] {
                                                        option { key: "{m}", value: m, selected: gamemode() == m, "{gamemode_label(m)}" }
                                                    }
                                                }
                                            }
                                            Field { label: "Seed", hint: "Leer = zufällige Welt.",
                                                input { class: INPUT, value: "{seed}", oninput: move |e| seed.set(e.value()) }
                                            }
                                            div { class: "sm:col-span-2",
                                                CheckRow {
                                                    checked: online_mode(),
                                                    onchange: move |v| online_mode.set(v),
                                                    label: "Online-Modus",
                                                    hint: "Prüft Konten bei Mojang. Nur ausschalten, wenn du weisst, was du tust.",
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            CheckRow {
                                checked: auto_start(),
                                onchange: move |v| auto_start.set(v),
                                label: "Automatisch starten",
                                hint: "Nach der Installation und wenn das Panel startet.",
                            }
                            if g == Game::Minecraft {
                                div { class: if eula() { "rounded-lg border p-3" } else { "rounded-lg border border-warning/30 bg-warning/10 p-3" },
                                    CheckRow {
                                        checked: eula(),
                                        onchange: move |v| eula.set(v),
                                        label: "Ich akzeptiere die Minecraft-EULA (Pflicht)",
                                        a {
                                            class: "w-fit text-xs underline text-muted-foreground hover:text-foreground",
                                            href: "https://aka.ms/MinecraftEULA",
                                            target: "_blank",
                                            rel: "noopener",
                                            "EULA lesen"
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Step::Summary => rsx! {
                        div { class: "{CARD} divide-y",
                            SummaryRow { label: "Spiel", value: g.label() }
                            if g == Game::Minecraft {
                                SummaryRow { label: "Software", value: f.label() }
                            }
                            SummaryRow {
                                label: "Version",
                                value: {
                                    let v = version().unwrap_or_else(|| "neueste stabile".into());
                                    if needs_loader(f) { format!("{v} · Loader {}", loader().unwrap_or_else(|| "neueste".into())) } else { v }
                                },
                            }
                            SummaryRow { label: "Name", value: name().trim().to_string() }
                            if g == Game::Minecraft {
                                SummaryRow { label: "Arbeitsspeicher", value: gb(memory()) }
                            }
                            SummaryRow { label: "Port", value: port() }
                            if let Some(m) = non_empty(motd()) {
                                SummaryRow { label: if g == Game::Minecraft { "MOTD" } else { "Beschreibung" }, value: m }
                            }
                            SummaryRow { label: "Max. Spieler", value: max_players() }
                            if g == Game::Minecraft {
                                SummaryRow { label: "Schwierigkeit", value: difficulty_label(&difficulty()) }
                                SummaryRow { label: "Spielmodus", value: gamemode_label(&gamemode()) }
                                SummaryRow { label: "Online-Modus", value: if online_mode() { "an" } else { "aus" } }
                                if let Some(s) = non_empty(seed()) {
                                    SummaryRow { label: "Seed", value: s }
                                }
                                SummaryRow { label: "EULA", value: if eula() { "akzeptiert" } else { "nicht akzeptiert" } }
                            }
                            SummaryRow { label: "Automatisch starten", value: if auto_start() { "ja" } else { "nein" } }
                        }
                        Notice { "Nach dem Erstellen lädt das Panel die Server-Dateien herunter. Den Fortschritt siehst du in der Konsole des Servers." }
                        if let Some(e) = error() {
                            ErrorBox { message: e }
                        }
                    },
                }

                div { class: "flex items-center justify-between gap-2 border-t pt-4",
                    if idx > 0 {
                        Button { variant: ButtonVariant::Outline, disabled: busy(), onclick: move |_| go_back(),
                            icons::ChevronLeft { class: "size-4" }
                            "Zurück"
                        }
                    } else {
                        span {}
                    }
                    if cur == Step::Summary {
                        Button { button_type: "submit", disabled: !can_next,
                            if busy() {
                                icons::LoaderCircle { class: "size-4 animate-spin" }
                                "Wird erstellt …"
                            } else {
                                icons::Rocket { class: "size-4" }
                                "Server erstellen"
                            }
                        }
                    } else {
                        Button { button_type: "submit", disabled: !can_next,
                            "Weiter"
                            icons::ChevronRight { class: "size-4" }
                        }
                    }
                }
            }
        }
    }
}

/// Grosse Auswahlkarte (Spiel, Factorio-Kanal).
#[component]
fn GameCard(
    selected: bool,
    #[props(into)] title: String,
    #[props(into)] text: String,
    #[props(into, optional)] tag: Option<String>,
    icon: Element,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if selected {
                "flex items-start gap-4 rounded-xl border border-foreground/40 bg-accent/40 p-5 text-left shadow-sm ring-1 ring-foreground/20"
            } else {
                "flex items-start gap-4 rounded-xl border bg-card p-5 text-left shadow-sm transition-colors hover:bg-accent/40"
            },
            onclick: move |e| onclick.call(e),
            div { class: "flex size-12 shrink-0 items-center justify-center rounded-lg bg-muted", {icon} }
            div { class: "flex min-w-0 flex-col gap-1",
                div { class: "flex flex-wrap items-center gap-2",
                    span { class: "text-base font-semibold", "{title}" }
                    if let Some(t) = tag {
                        Tag { "{t}" }
                    }
                }
                p { class: "text-sm text-muted-foreground", "{text}" }
            }
        }
    }
}

#[component]
fn SummaryRow(#[props(into)] label: String, #[props(into)] value: String) -> Element {
    rsx! {
        div { class: "flex flex-wrap items-center justify-between gap-x-4 gap-y-1 px-4 py-2.5 text-sm",
            span { class: "text-muted-foreground", "{label}" }
            span { class: "font-medium break-all", "{value}" }
        }
    }
}
