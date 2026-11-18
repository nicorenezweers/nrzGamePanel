//! Konsole: Live-Ausgabe (ANSI-Farben), Befehlseingabe mit Verlauf, Schnellbefehle, Filter.

use dioxus::prelude::*;
use nrz_gp_common::{ConsoleLine, ConsoleSource, Game};

use crate::{
    components::{AnsiLine, INPUT, fmt_time_ms},
    layout::use_server,
};

/// Konsolen-Ansicht (auch auf der Übersicht eingebettet, dann `compact`).
#[component]
pub fn ConsoleView(#[props(default)] compact: bool) -> Element {
    let ctx = use_server();
    let mut input = use_signal(String::new);
    let mut history = use_signal(Vec::<String>::new);
    let mut hist_pos = use_signal(|| None::<usize>);
    let mut follow = use_signal(|| true);
    let mut filter = use_signal(String::new);
    let mut show_time = use_signal(|| false);
    let el_id = if compact { "console-compact" } else { "console-main" };

    let lines = ctx.console.read().clone();
    let f = filter().to_lowercase();
    let shown: Vec<ConsoleLine> = if f.is_empty() {
        lines.iter().rev().take(if compact { 200 } else { 3000 }).rev().cloned().collect()
    } else {
        lines.iter().filter(|l| l.text.to_lowercase().contains(&f)).cloned().collect()
    };
    let count = shown.len();

    // Automatisch nach unten scrollen
    use_effect(use_reactive!(|count| {
        let _ = count;
        if follow() {
            if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(el_id)) {
                el.set_scroll_top(el.scroll_height());
            }
        }
    }));

    let mut send = move || {
        let cmd = input().trim().to_string();
        if cmd.is_empty() {
            return;
        }
        ctx.send(&cmd);
        let mut h = history.write();
        if h.last() != Some(&cmd) {
            h.push(cmd);
        }
        hist_pos.set(None);
        input.set(String::new());
        follow.set(true);
    };

    let online = ctx.status.read().is_online();
    let quick: Vec<(&str, &str)> = match ctx.game() {
        Game::Minecraft => vec![
            ("list", "Spielerliste"),
            ("save-all", "Speichern"),
            ("time set day", "Tag"),
            ("weather clear", "Sonne"),
            ("difficulty normal", "Normal"),
            ("tps", "TPS"),
        ],
        Game::Factorio => vec![("players online", "Spieler"), ("server-save", "Speichern"), ("time", "Spielzeit"), ("version", "Version")],
    };
    let height = if compact { "h-72" } else { "h-[calc(100dvh-20rem)] min-h-80" };

    rsx! {
        div { class: "flex flex-col overflow-hidden rounded-xl border bg-card shadow-sm",
            if !compact {
                div { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2",
                    div { class: "relative min-w-40 flex-1",
                        icons::Search { class: "pointer-events-none absolute left-2.5 top-2.5 size-4 text-muted-foreground" }
                        input { class: "{INPUT} pl-8 h-9", placeholder: "Konsole durchsuchen …", value: "{filter}", oninput: move |e| filter.set(e.value()) }
                    }
                    button {
                        class: if show_time() { "rounded-md bg-accent px-2.5 py-1.5 text-xs" } else { "rounded-md px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-accent" },
                        onclick: move |_| show_time.toggle(),
                        icons::Clock { class: "inline size-3.5 mr-1" }
                        "Zeit"
                    }
                    button {
                        class: if follow() { "rounded-md bg-accent px-2.5 py-1.5 text-xs" } else { "rounded-md px-2.5 py-1.5 text-xs text-muted-foreground hover:bg-accent" },
                        onclick: move |_| follow.toggle(),
                        icons::ArrowDownToLine { class: "inline size-3.5 mr-1" }
                        "Mitlaufen"
                    }
                }
            }
            div {
                id: el_id,
                class: "{height} overflow-y-auto bg-background/60 px-3 py-2 font-mono text-[12.5px] leading-relaxed",
                onscroll: move |_| {
                    if let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(el_id)) {
                        let at_bottom = el.scroll_height() - el.scroll_top() - el.client_height() < 40;
                        if at_bottom != follow() {
                            follow.set(at_bottom);
                        }
                    }
                },
                if shown.is_empty() {
                    div { class: "py-8 text-center text-muted-foreground",
                        if online { "Warte auf Ausgabe …" } else { "Server ist gestoppt. Starte ihn, um die Ausgabe zu sehen." }
                    }
                }
                for l in shown {
                    div {
                        key: "{l.seq}",
                        class: match l.source {
                            ConsoleSource::Input => "console-line text-info",
                            ConsoleSource::Panel => "console-line text-muted-foreground italic",
                            ConsoleSource::Stderr => "console-line text-destructive/90",
                            ConsoleSource::Stdout => if l.text.contains("WARN") { "console-line text-warning" } else if l.text.contains("ERROR") || l.text.contains("Exception") { "console-line text-destructive" } else { "console-line" },
                        },
                        if show_time() {
                            span { class: "mr-2 text-muted-foreground/70", "{fmt_time_ms(l.ts)}" }
                        }
                        AnsiLine { text: l.text.clone() }
                    }
                }
            }
            div { class: "flex items-center gap-2 border-t px-3 py-2",
                span { class: "font-mono text-sm text-muted-foreground", if ctx.game() == Game::Factorio { "/" } else { ">" } }
                input {
                    class: "h-9 flex-1 bg-transparent font-mono text-sm outline-none placeholder:text-muted-foreground disabled:opacity-50",
                    placeholder: if online { "Befehl eingeben … (↑ ↓ für Verlauf)" } else { "Server läuft nicht" },
                    disabled: !online,
                    value: "{input}",
                    oninput: move |e| input.set(e.value()),
                    onkeydown: move |e| {
                        match e.key() {
                            Key::Enter => send(),
                            Key::ArrowUp => {
                                e.prevent_default();
                                let h = history.read();
                                if h.is_empty() { return; }
                                let pos = hist_pos().map_or(h.len() - 1, |p| p.saturating_sub(1));
                                input.set(h[pos].clone());
                                drop(h);
                                hist_pos.set(Some(pos));
                            }
                            Key::ArrowDown => {
                                e.prevent_default();
                                let h = history.read();
                                if let Some(p) = hist_pos() {
                                    if p + 1 < h.len() {
                                        input.set(h[p + 1].clone());
                                        drop(h);
                                        hist_pos.set(Some(p + 1));
                                    } else {
                                        drop(h);
                                        hist_pos.set(None);
                                        input.set(String::new());
                                    }
                                }
                            }
                            _ => {}
                        }
                    },
                }
                button {
                    class: "rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40",
                    disabled: !online,
                    aria_label: "Senden",
                    onclick: move |_| send(),
                    icons::SendHorizontal { class: "size-4" }
                }
            }
            if !compact && online {
                div { class: "flex flex-wrap gap-1.5 border-t px-3 py-2",
                    for (cmd, label) in quick {
                        button {
                            key: "{cmd}",
                            class: "rounded-md border px-2 py-1 text-xs text-muted-foreground hover:bg-accent hover:text-foreground",
                            title: "{cmd}",
                            onclick: move |_| ctx.send(cmd),
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn ServerConsole(id: String) -> Element {
    let _ = id;
    rsx! {
        div { class: "mx-auto max-w-7xl p-4 md:p-8",
            ConsoleView {}
        }
    }
}
