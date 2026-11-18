//! Integrationen: MCP-Server (KI-Assistenten), API-Tokens, REST-API, Webhooks.

use dioxus::prelude::*;
use nrz_gp_common::{ApiToken, CreateToken, CreatedToken};

use crate::{
    api,
    components::{
        CodeBlock, ConfirmDialog, CopyField, ErrorBox, INPUT, Loading, Notice, Segmented, fmt_ago, fmt_datetime,
        ui::{
            app_shell::{PageContainer, PageHeader, PageSection},
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    store::{toast, toast_err},
};

const TOOLS: &[(&str, &str)] = &[
    ("list_servers", "Alle Server mit Status, Spielern, CPU und RAM"),
    ("get_server", "Details eines Servers inkl. Warnungen und Konfiguration"),
    ("power", "Server starten, stoppen, neu starten oder beenden"),
    ("send_command", "Konsolenbefehl senden und Antwort lesen"),
    ("read_console", "Letzte Konsolenzeilen lesen"),
    ("list_events", "Ereignisse: Beitritte, Chat, Tode, Abstürze"),
    ("list_files", "Dateien eines Ordners im Server-Verzeichnis"),
    ("read_file", "Textdatei lesen"),
    ("write_file", "Textdatei schreiben"),
    ("list_players", "Minecraft: Spieler online, bekannt, OPs, Whitelist, Banns"),
    ("get_player", "Minecraft: Spielerdetails mit Inventar und Endertruhe"),
    ("player_action", "Minecraft: Kick, Bann, OP, Whitelist, Teleport …"),
    ("give_item", "Minecraft: Item an einen Spieler geben"),
    ("player_positions", "Minecraft: Positionen aller Spieler"),
    ("create_backup", "Backup erstellen (Welten oder ganzer Ordner)"),
];

fn origin() -> String {
    web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default()
}

#[component]
pub fn Integrations() -> Element {
    let base = origin();
    let mcp_url = format!("{base}/mcp");

    let mut reload = use_signal(|| 0u32);
    let tokens = use_resource(move || {
        let _ = reload();
        async move { api::get::<Vec<ApiToken>>("/api/tokens").await }
    });
    let mut new_name = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut created = use_signal(|| None::<CreatedToken>);
    let mut deleting = use_signal(|| None::<ApiToken>);
    let mut snippet = use_signal(|| "claude".to_string());

    let mut create = move || {
        let name = new_name().trim().to_string();
        let name = if name.is_empty() { "MCP".to_string() } else { name };
        creating.set(true);
        spawn(async move {
            match api::post::<_, CreatedToken>("/api/tokens", &CreateToken { name }).await {
                Ok(t) => {
                    created.set(Some(t));
                    new_name.set(String::new());
                    reload += 1;
                }
                Err(e) => toast_err(e),
            }
            creating.set(false);
        });
    };

    let confirm_delete = move |_| {
        let Some(t) = deleting() else { return };
        deleting.set(None);
        spawn(async move {
            match api::delete::<nrz_gp_common::Ok>(&format!("/api/tokens/{}", t.id)).await {
                Ok(_) => {
                    toast("Token widerrufen");
                    if created().is_some_and(|c| c.token.id == t.id) {
                        created.set(None);
                    }
                    reload += 1;
                }
                Err(e) => toast_err(e),
            }
        });
    };

    let token_text = created().map(|c| c.secret).unwrap_or_else(|| "<token>".into());
    let claude_cmd = format!("claude mcp add --transport http nrzgamepanel {mcp_url} --header \"Authorization: Bearer {token_text}\"");
    let json_cfg = format!(
        "{{\n  \"mcpServers\": {{\n    \"nrzgamepanel\": {{\n      \"type\": \"http\",\n      \"url\": \"{mcp_url}\",\n      \"headers\": {{\n        \"Authorization\": \"Bearer {token_text}\"\n      }}\n    }}\n  }}\n}}"
    );
    let curl = format!("curl -H \"Authorization: Bearer {token_text}\" {base}/api/servers");

    rsx! {
        PageContainer {
            PageHeader {
                title: "Integrationen",
                description: "KI-Assistenten, Skripte und andere Dienste mit dem Panel verbinden.",
            }
            div { class: "flex flex-col gap-6",
                PageSection {
                    title: "MCP-Server",
                    description: "Über das Model Context Protocol können KI-Assistenten wie Claude deine Server verwalten: Status abfragen, starten, Befehle senden, Dateien lesen, Spieler verwalten und Backups erstellen.",
                    div { class: "flex flex-col gap-5",
                        div { class: "flex flex-col gap-1.5",
                            span { class: "text-sm font-medium", "Adresse" }
                            CopyField { value: mcp_url.clone() }
                            span { class: "text-xs text-muted-foreground", "Streamable HTTP · Anmeldung mit einem API-Token (unten) als Bearer-Token." }
                        }
                        div { class: "flex flex-col gap-2",
                            div { class: "flex flex-wrap items-center justify-between gap-2",
                                span { class: "text-sm font-medium", "Einrichten" }
                                Segmented {
                                    items: vec![("claude".to_string(), "Claude Code".to_string()), ("json".to_string(), "JSON".to_string())],
                                    active: snippet(),
                                    onchange: move |v| snippet.set(v),
                                }
                            }
                            if snippet() == "claude" {
                                CodeBlock { label: "Terminal", code: claude_cmd }
                            } else {
                                CodeBlock { label: "mcpServers (z. B. .mcp.json oder Claude Desktop)", code: json_cfg }
                            }
                            if created().is_none() {
                                p { class: "text-xs text-muted-foreground", "Ersetze <token> durch ein API-Token. Erstellst du unten ein neues, wird es hier direkt eingesetzt." }
                            }
                        }
                        div { class: "flex flex-col gap-2",
                            span { class: "text-sm font-medium", "Verfügbare Werkzeuge" }
                            div { class: "grid gap-x-6 gap-y-1.5 sm:grid-cols-2",
                                for (name, desc) in TOOLS.iter() {
                                    div { key: "{name}", class: "flex min-w-0 flex-col py-1",
                                        code { class: "font-mono text-xs", "{name}" }
                                        span { class: "text-xs text-muted-foreground", "{desc}" }
                                    }
                                }
                            }
                        }
                    }
                }

                PageSection {
                    title: "API-Tokens",
                    description: "Tokens gelten für den MCP-Server und alle /api-Routen. Widerrufe Tokens, die du nicht mehr brauchst.",
                    div { class: "flex flex-col gap-4",
                        if let Some(c) = created() {
                            div { class: "flex flex-col gap-2 rounded-lg border border-success/30 bg-success/10 p-3",
                                div { class: "flex items-center gap-2 text-sm font-medium",
                                    icons::CircleCheck { class: "size-4 text-success" }
                                    "Token „{c.token.name}“ erstellt"
                                }
                                CopyField { value: c.secret.clone() }
                                p { class: "text-xs text-muted-foreground", "Kopiere es jetzt – aus Sicherheitsgründen wird es nur dieses eine Mal angezeigt." }
                            }
                        }
                        form {
                            class: "flex flex-wrap gap-2",
                            onsubmit: move |e| {
                                e.prevent_default();
                                create();
                            },
                            input {
                                class: "{INPUT} min-w-0 flex-1 sm:max-w-xs",
                                placeholder: "Name, z. B. Claude Code",
                                value: "{new_name}",
                                oninput: move |e| new_name.set(e.value()),
                            }
                            Button { button_type: "submit", disabled: creating(),
                                if creating() {
                                    icons::LoaderCircle { class: "size-4 animate-spin" }
                                } else {
                                    icons::KeyRound { class: "size-4" }
                                }
                                "Token erstellen"
                            }
                        }
                        match &*tokens.read() {
                            None => rsx! { Loading { text: "Lade Tokens …" } },
                            Some(Err(e)) => rsx! { ErrorBox { message: e.clone() } },
                            Some(Ok(list)) if list.is_empty() => rsx! {
                                p { class: "rounded-lg border border-dashed px-4 py-6 text-center text-sm text-muted-foreground", "Noch keine Tokens." }
                            },
                            Some(Ok(list)) => rsx! {
                                div { class: "divide-y rounded-lg border",
                                    for t in list.iter().cloned() {
                                        {
                                            let t2 = t.clone();
                                            rsx! {
                                                div { key: "{t.id}", class: "flex flex-wrap items-center gap-3 px-3 py-2.5",
                                                    icons::KeyRound { class: "size-4 shrink-0 text-muted-foreground" }
                                                    div { class: "min-w-0 flex-1",
                                                        div { class: "flex flex-wrap items-center gap-2",
                                                            span { class: "text-sm font-medium", "{t.name}" }
                                                            code { class: "font-mono text-xs text-muted-foreground", "{t.prefix}…" }
                                                        }
                                                        div { class: "text-xs text-muted-foreground",
                                                            "Erstellt {fmt_datetime(t.created_at)} · "
                                                            match t.last_used {
                                                                Some(u) => rsx! { "zuletzt benutzt {fmt_ago(u)}" },
                                                                None => rsx! { "nie benutzt" },
                                                            }
                                                        }
                                                    }
                                                    Button {
                                                        size: ButtonSize::Sm,
                                                        variant: ButtonVariant::Ghost,
                                                        onclick: move |_| deleting.set(Some(t2.clone())),
                                                        icons::Trash2 { class: "size-4 text-destructive" }
                                                        "Widerrufen"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                }

                PageSection {
                    title: "REST-API",
                    description: "Alle Funktionen der Oberfläche sind auch per HTTP erreichbar. Ein API-Token als Bearer-Token funktioniert für alle /api-Routen.",
                    div { class: "flex flex-col gap-3",
                        CodeBlock { label: "Beispiel: Server auflisten", code: curl }
                        p { class: "text-xs text-muted-foreground",
                            "Antworten sind JSON; Fehler kommen mit HTTP-Status und "
                            code { class: "font-mono", "{{\"error\": \"…\"}}" }
                            "."
                        }
                    }
                }

                PageSection {
                    title: "Webhooks",
                    description: "Benachrichtigungen an Discord oder eigene Dienste.",
                    Notice {
                        "Webhooks richtest du pro Server über "
                        span { class: "font-medium", "Automationen" }
                        " ein: Auslöser wählen (z. B. Absturz oder Spielerbeitritt) und die Aktion „Webhook / Discord“ hinzufügen. Die Vorlage „Discord bei Absturz“ ist schon vorbereitet."
                    }
                }
            }
        }

        if let Some(t) = deleting() {
            ConfirmDialog {
                title: "Token widerrufen?",
                message: format!("„{}“ funktioniert danach nicht mehr. Anwendungen, die es nutzen, verlieren den Zugriff.", t.name),
                confirm_label: "Widerrufen",
                destructive: true,
                onconfirm: confirm_delete,
                oncancel: move |_| deleting.set(None),
            }
        }
    }
}
