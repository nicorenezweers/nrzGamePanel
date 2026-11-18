//! Weltkarte aus Region-Kacheln (512×512 Blöcke je Bild): verschieben, zoomen (Mausrad, Finger, Knöpfe),
//! Spieler-Marker, Spawn, Koordinaten unter dem Mauszeiger.

use dioxus::prelude::*;
use nrz_gp_common::{MapPlayer, MapRegions, WorldInfo, routes};
use wasm_bindgen::JsCast;

use super::players::dimension_label;
use crate::{
    Route, api,
    components::{
        ErrorBox, ICON_BUTTON, Loading, Notice, PlayerHead, SELECT, Segmented,
        ui::{
            app_shell::EmptyState,
            button::{Button, ButtonSize, ButtonVariant},
        },
    },
    layout::use_server,
};

/// Zentrum für die nächste Kartenansicht `(x, z, dimension)` – gesetzt z. B. von der Spielerseite.
pub static MAP_FOCUS: GlobalSignal<Option<(f64, f64, String)>> = Signal::global(|| None);

const SURFACE: &str = "map-surface";
const MIN_SCALE: f64 = 0.125;
const MAX_SCALE: f64 = 16.0;
const REGION: f64 = 512.0;

/// `?x=&z=&dim=` aus der Adresszeile.
fn query_focus() -> Option<(f64, f64, Option<String>)> {
    let search = web_sys::window()?.location().search().ok()?;
    let (mut x, mut z, mut dim) = (None, None, None);
    for pair in search.trim_start_matches('?').split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = js_sys::decode_uri_component(v).ok().and_then(|s| s.as_string()).unwrap_or_else(|| v.to_string());
        match k {
            "x" => x = v.parse::<f64>().ok(),
            "z" => z = v.parse::<f64>().ok(),
            "dim" if !v.is_empty() => dim = Some(v),
            _ => {}
        }
    }
    Some((x?, z?, dim))
}

/// Position und Grösse der Kartenfläche im Fenster `(links, oben, breite, höhe)`.
fn surface_rect() -> Option<(f64, f64, f64, f64)> {
    let el = web_sys::window()?.document()?.get_element_by_id(SURFACE)?;
    let r = el.get_bounding_client_rect();
    Some((r.left(), r.top(), r.width(), r.height()))
}

fn fmt_scale(s: f64) -> String {
    if s >= 1.0 { format!("{s:.0}×") } else { format!("1:{:.0}", 1.0 / s) }
}

#[component]
pub fn ServerMap(id: String) -> Element {
    let ctx = use_server();
    let nav = navigator();

    // Startfokus: von der Spielerseite oder aus ?x=&z=
    let initial = use_hook(|| {
        MAP_FOCUS.write().take().map(|(x, z, d)| (x, z, Some(d))).or_else(query_focus)
    });
    let mut world = use_signal(|| None::<String>);
    let mut dim = use_signal(|| initial.as_ref().and_then(|f| f.2.clone()).unwrap_or_else(|| "minecraft:overworld".into()));
    let mut cx = use_signal(|| initial.as_ref().map_or(0.0, |f| f.0));
    let mut cz = use_signal(|| initial.as_ref().map_or(0.0, |f| f.1));
    let mut scale = use_signal(|| if initial.is_some() { 2.0 } else { 1.0 });
    let mut need_center = use_signal(|| initial.is_none());
    let mut size = use_signal(|| (800.0f64, 560.0f64));
    let mut buster = use_signal(|| 0u64);
    let mut pointers = use_signal(Vec::<(i32, f64, f64)>::new);
    let mut hover = use_signal(|| None::<(f64, f64)>);
    let mut players = use_signal(Vec::<MapPlayer>::new);

    let worlds = use_resource(move || {
        let id = ctx.id();
        async move { api::get::<Vec<WorldInfo>>(&routes::server(&id, "mc/worlds")).await }
    });
    let regions = use_resource(move || {
        let id = ctx.id();
        let w = world();
        let d = dim();
        let t = buster();
        async move {
            let Some(w) = w else { return Ok(None) };
            let mut url = format!("{}?dim={}", routes::server(&id, &format!("mc/map/{}/regions", routes::enc(&w))), routes::enc(&d));
            if t > 0 {
                url.push_str(&format!("&t={t}"));
            }
            api::get::<MapRegions>(&url).await.map(Some)
        }
    });

    // Aktive Welt wählen, sobald die Liste da ist
    use_effect(move || {
        if let Some(Ok(list)) = &*worlds.read() {
            if world.peek().is_none() {
                if let Some(w) = list.iter().find(|w| w.active).or_else(|| list.first()) {
                    let d = dim.peek().clone();
                    if !w.dimensions.is_empty() && !w.dimensions.iter().any(|x| x.id == d) {
                        dim.set(w.dimensions[0].id.clone());
                    }
                    world.set(Some(w.name.clone()));
                }
            }
        }
    });
    // Beim ersten Laden bzw. nach Welt-/Dimensionswechsel zentrieren
    use_effect(move || {
        if let Some(Ok(Some(r))) = &*regions.read() {
            if *need_center.peek() {
                let (x, z) = r.spawn.map(|(x, z)| (x as f64, z as f64)).unwrap_or_else(|| {
                    if r.regions.is_empty() {
                        (0.0, 0.0)
                    } else {
                        let n = r.regions.len() as f64;
                        let sx: f64 = r.regions.iter().map(|(x, _)| *x as f64).sum();
                        let sz: f64 = r.regions.iter().map(|(_, z)| *z as f64).sum();
                        ((sx / n + 0.5) * REGION, (sz / n + 0.5) * REGION)
                    }
                });
                cx.set(x);
                cz.set(z);
                need_center.set(false);
            }
        }
    });
    // Spieler alle 5 s
    use_future(move || async move {
        loop {
            let id = ctx.id.peek().clone();
            if let Ok(p) = api::get::<Vec<MapPlayer>>(&routes::server(&id, "mc/map/players")).await {
                players.set(p);
            }
            gloo_timers::future::TimeoutFuture::new(5000).await;
        }
    });

    // Zoom um einen Punkt (Pixel relativ zur Fläche)
    let mut zoom_at = move |mx: f64, my: f64, factor: f64| {
        let (w, h) = *size.peek();
        let s = *scale.peek();
        let ns = (s * factor).clamp(MIN_SCALE, MAX_SCALE);
        if (ns - s).abs() < f64::EPSILON {
            return;
        }
        let wx = *cx.peek() + (mx - w / 2.0) / s;
        let wz = *cz.peek() + (my - h / 2.0) / s;
        cx.set(wx - (mx - w / 2.0) / ns);
        cz.set(wz - (my - h / 2.0) / ns);
        scale.set(ns);
    };
    let mut center_on = move |x: f64, z: f64| {
        cx.set(x);
        cz.set(z);
        if *scale.peek() < 1.0 {
            scale.set(1.0);
        }
    };

    let worlds_state = worlds.read().clone();
    let world_list = match &worlds_state {
        None => return rsx! { div { class: "mx-auto max-w-7xl p-4 md:p-8", Loading { text: "Lade Welten …" } } },
        Some(Err(e)) => return rsx! { div { class: "mx-auto max-w-7xl p-4 md:p-8", ErrorBox { message: e.clone() } } },
        Some(Ok(l)) => l.clone(),
    };
    if world_list.is_empty() {
        return rsx! {
            div { class: "mx-auto max-w-7xl p-4 md:p-8",
                EmptyState {
                    title: "Noch keine Welt",
                    description: "Starte den Server einmal, damit Minecraft eine Welt erzeugt. Danach erscheint hier die Karte.",
                    icon: rsx! { icons::Map {} },
                }
            }
        };
    }
    let cur_world = world().unwrap_or_default();
    let winfo = world_list.iter().find(|w| w.name == cur_world).cloned();
    let mut dims: Vec<String> = winfo.as_ref().map(|w| w.dimensions.iter().map(|d| d.id.clone()).collect()).unwrap_or_default();
    let cur_dim = dim();
    if !dims.contains(&cur_dim) {
        dims.push(cur_dim.clone());
    }
    let dim_items: Vec<(String, String)> = dims.iter().map(|d| (d.clone(), dimension_label(d))).collect();

    let regions_state = regions.read().clone();
    let (w, h) = size();
    let s = scale();
    let (x, z) = (cx(), cz());
    let to_screen = move |wx: f64, wz: f64| ((wx - x) * s + w / 2.0, (wz - z) * s + h / 2.0);

    let (tiles, spawn, textured, total) = match &regions_state {
        Some(Ok(Some(r))) => {
            let (x0, x1) = (x - w / 2.0 / s, x + w / 2.0 / s);
            let (z0, z1) = (z - h / 2.0 / s, z + h / 2.0 / s);
            let (rx0, rx1) = ((x0 / REGION).floor() as i32, (x1 / REGION).floor() as i32);
            let (rz0, rz1) = ((z0 / REGION).floor() as i32, (z1 / REGION).floor() as i32);
            let visible: Vec<(i32, i32)> = r.regions.iter().copied().filter(|(rx, rz)| (rx0..=rx1).contains(rx) && (rz0..=rz1).contains(rz)).collect();
            (visible, r.spawn.or_else(|| winfo.as_ref().and_then(|w| w.spawn).map(|(x, _, z)| (x, z))), r.textured, r.regions.len())
        }
        _ => (Vec::new(), None, true, 0),
    };
    let pix = if s >= 1.0 { "pixelated" } else { "" };
    let tile_base = format!("{}/tile", routes::server(&id, &format!("mc/map/{}", routes::enc(&cur_world))));
    let t = buster();
    let dim_q = routes::enc(&cur_dim);

    let same_dim = |p: &MapPlayer| p.position.dimension == cur_dim || (p.position.dimension.is_empty() && cur_dim == "minecraft:overworld");
    let mut plist = players();
    plist.sort_by(|a, b| b.online.cmp(&a.online).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let markers: Vec<MapPlayer> = plist.iter().filter(|p| same_dim(p)).cloned().collect();

    rsx! {
        div { class: "mx-auto flex max-w-7xl flex-col gap-4 p-4 md:p-8",
            // Werkzeugleiste
            div { class: "flex flex-wrap items-center gap-2",
                if world_list.len() > 1 {
                    select {
                        class: "{SELECT} w-auto",
                        aria_label: "Welt",
                        value: "{cur_world}",
                        onchange: move |e| {
                            world.set(Some(e.value()));
                            need_center.set(true);
                        },
                        for wi in world_list.iter() {
                            option { key: "{wi.name}", value: "{wi.name}", selected: wi.name == cur_world,
                                if wi.active { "{wi.name} (aktiv)" } else { "{wi.name}" }
                            }
                        }
                    }
                }
                Segmented {
                    items: dim_items,
                    active: cur_dim.clone(),
                    onchange: move |d: String| {
                        dim.set(d);
                        need_center.set(true);
                    },
                }
                div { class: "flex-1" }
                if spawn.is_some() {
                    Button {
                        size: ButtonSize::Sm,
                        variant: ButtonVariant::Outline,
                        onclick: move |_| if let Some((sx, sz)) = spawn { center_on(sx as f64, sz as f64) },
                        icons::Flag { class: "size-4" }
                        "Spawn"
                    }
                }
                Button {
                    size: ButtonSize::Sm,
                    variant: ButtonVariant::Outline,
                    onclick: move |_| buster.set(js_sys::Date::now() as u64),
                    icons::RefreshCw { class: "size-4" }
                    "Karte neu laden"
                }
            }

            if !textured {
                Notice { "Vereinfachte Farben: Die Blockfarben stammen aus einer eingebauten Tabelle, weil die Client-Texturen noch nicht verfügbar sind." }
            }

            div { class: "grid gap-4 lg:grid-cols-[1fr_16rem]",
                // Karte
                div {
                    id: SURFACE,
                    class: "relative h-[calc(100dvh-18rem)] min-h-96 touch-none select-none overflow-hidden rounded-xl border bg-muted/30 shadow-sm cursor-grab active:cursor-grabbing",
                    tabindex: "0",
                    onmounted: move |_| {
                        if let Some((_, _, w, h)) = surface_rect() {
                            size.set((w, h));
                        }
                    },
                    onresize: move |e| {
                        if let Ok(sz) = e.get_content_box_size() {
                            size.set((sz.width, sz.height));
                        }
                    },
                    onkeydown: move |e| {
                        let step = 64.0 / *scale.peek();
                        match e.key() {
                            Key::ArrowLeft => cx -= step,
                            Key::ArrowRight => cx += step,
                            Key::ArrowUp => cz -= step,
                            Key::ArrowDown => cz += step,
                            Key::Character(c) if c == "+" => {
                                let (w, h) = *size.peek();
                                zoom_at(w / 2.0, h / 2.0, 1.5);
                            }
                            Key::Character(c) if c == "-" => {
                                let (w, h) = *size.peek();
                                zoom_at(w / 2.0, h / 2.0, 1.0 / 1.5);
                            }
                            _ => return,
                        }
                        e.prevent_default();
                    },
                    onwheel: move |e| {
                        e.prevent_default();
                        let d = e.delta();
                        let dy = match d {
                            dioxus::html::geometry::WheelDelta::Pixels(v) => v.y,
                            dioxus::html::geometry::WheelDelta::Lines(v) => v.y * 16.0,
                            dioxus::html::geometry::WheelDelta::Pages(v) => v.y * 400.0,
                        };
                        if dy == 0.0 {
                            return;
                        }
                        let f = (-dy * 0.002).exp().clamp(0.5, 2.0);
                        let c = e.client_coordinates();
                        if let Some((l, t, _, _)) = surface_rect() {
                            zoom_at(c.x - l, c.y - t, f);
                        }
                    },
                    onpointerdown: move |e| {
                        let c = e.client_coordinates();
                        if let Some(pe) = e.downcast::<web_sys::PointerEvent>() {
                            if let Some(el) = pe.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                                if el.id() == SURFACE {
                                    let _ = el.set_pointer_capture(pe.pointer_id());
                                }
                            }
                        }
                        let pid = e.pointer_id();
                        let mut p = pointers.write();
                        p.retain(|(i, _, _)| *i != pid);
                        p.push((pid, c.x, c.y));
                    },
                    onpointermove: move |e| {
                        let c = e.client_coordinates();
                        let rect = surface_rect();
                        let pid = e.pointer_id();
                        let ps = pointers.peek().clone();
                        if let Some(idx) = ps.iter().position(|(i, _, _)| *i == pid) {
                            let s = *scale.peek();
                            if ps.len() == 1 {
                                let (_, ox, oy) = ps[0];
                                cx -= (c.x - ox) / s;
                                cz -= (c.y - oy) / s;
                            } else if ps.len() >= 2 {
                                let other = ps[if idx == 0 { 1 } else { 0 }];
                                let old = ps[idx];
                                let d_old = ((old.1 - other.1).powi(2) + (old.2 - other.2).powi(2)).sqrt();
                                let d_new = ((c.x - other.1).powi(2) + (c.y - other.2).powi(2)).sqrt();
                                if d_old > 4.0 {
                                    if let Some((l, t, _, _)) = rect {
                                        zoom_at((c.x + other.1) / 2.0 - l, (c.y + other.2) / 2.0 - t, d_new / d_old);
                                    }
                                }
                            }
                            pointers.write()[idx] = (pid, c.x, c.y);
                        }
                        if let Some((l, t, w, h)) = rect {
                            if (w, h) != *size.peek() && w > 0.0 {
                                size.set((w, h));
                            }
                            let s = *scale.peek();
                            hover.set(Some((*cx.peek() + (c.x - l - w / 2.0) / s, *cz.peek() + (c.y - t - h / 2.0) / s)));
                        }
                    },
                    onpointerup: move |e| {
                        let pid = e.pointer_id();
                        pointers.write().retain(|(i, _, _)| *i != pid);
                    },
                    onpointercancel: move |e| {
                        let pid = e.pointer_id();
                        pointers.write().retain(|(i, _, _)| *i != pid);
                    },
                    onpointerleave: move |_| {
                        if pointers.peek().is_empty() {
                            hover.set(None);
                        }
                    },

                    // Kacheln
                    div { class: "pointer-events-none absolute inset-0",
                        for (rx, rz) in tiles.iter().copied() {
                            {
                                let (l, tp) = to_screen(rx as f64 * REGION, rz as f64 * REGION);
                                let (r, b) = to_screen((rx + 1) as f64 * REGION, (rz + 1) as f64 * REGION);
                                let (l, tp, r, b) = (l.floor(), tp.floor(), r.floor(), b.floor());
                                let src = if t > 0 { format!("{tile_base}/{rx}/{rz}?dim={dim_q}&t={t}") } else { format!("{tile_base}/{rx}/{rz}?dim={dim_q}") };
                                rsx! {
                                    img {
                                        key: "{rx},{rz}",
                                        class: "absolute max-w-none {pix}",
                                        style: "left: {l}px; top: {tp}px; width: {r - l}px; height: {b - tp}px",
                                        src: "{src}",
                                        alt: "",
                                        draggable: "false",
                                    }
                                }
                            }
                        }
                    }

                    // Spawn
                    if let Some((sx, sz)) = spawn {
                        {
                            let (px, py) = to_screen(sx as f64 + 0.5, sz as f64 + 0.5);
                            rsx! {
                                div {
                                    class: "pointer-events-none absolute flex -translate-x-1/2 -translate-y-full flex-col items-center",
                                    style: "left: {px}px; top: {py}px",
                                    span { class: "flex items-center gap-1 rounded-md border bg-popover/90 px-1.5 py-0.5 text-[11px] font-medium shadow-sm",
                                        icons::Flag { class: "size-3 text-info" }
                                        "Spawn"
                                    }
                                    span { class: "h-2 w-px bg-foreground/60" }
                                }
                            }
                        }
                    }

                    // Spieler
                    for p in markers.iter().cloned() {
                        {
                            let (px, py) = to_screen(p.position.x, p.position.z);
                            let target = p.uuid.clone().unwrap_or_else(|| p.name.clone());
                            let ring = if p.online { "ring-2 ring-success" } else { "opacity-60 grayscale" };
                            let z = if p.online { "z-20" } else { "z-10" };
                            rsx! {
                                div {
                                    key: "{p.name}",
                                    class: "absolute {z} flex -translate-x-1/2 -translate-y-1/2 flex-col items-center gap-0.5",
                                    style: "left: {px}px; top: {py}px",
                                    onpointerdown: move |e| e.stop_propagation(),
                                    Link {
                                        to: Route::PlayerPage { id: id.clone(), uuid: target },
                                        class: "flex flex-col items-center gap-0.5",
                                        title: "{p.name} · X {p.position.x:.0} · Y {p.position.y:.0} · Z {p.position.z:.0}",
                                        span { class: "rounded-sm bg-background {ring}", PlayerHead { player: p.name.clone(), size: 6 } }
                                        span { class: "rounded bg-popover/90 px-1 text-[11px] font-medium shadow-sm", "{p.name}" }
                                    }
                                }
                            }
                        }
                    }

                    // Zustände
                    match &regions_state {
                        None | Some(Ok(None)) => rsx! {
                            div { class: "absolute inset-0 flex items-center justify-center", Loading { text: "Lade Karte …" } }
                        },
                        Some(Err(e)) => rsx! {
                            div { class: "absolute inset-0 flex items-center justify-center p-6", ErrorBox { message: e.clone() } }
                        },
                        Some(Ok(Some(_))) if total == 0 => rsx! {
                            div { class: "absolute inset-0 flex items-center justify-center p-6",
                                div { class: "flex max-w-sm flex-col items-center gap-2 text-center",
                                    div { class: "grid size-12 place-items-center rounded-xl bg-muted text-muted-foreground", icons::Map { class: "size-6" } }
                                    p { class: "font-medium", "Keine Kartendaten" }
                                    p { class: "text-sm text-muted-foreground",
                                        "In dieser Dimension wurden noch keine Regionen erzeugt. Sobald jemand sie erkundet, erscheint sie hier."
                                    }
                                }
                            }
                        },
                        _ => rsx! {},
                    }

                    // Zoom-Knöpfe
                    div { class: "absolute top-3 right-3 z-30 flex flex-col overflow-hidden rounded-lg border bg-popover/90 shadow-sm",
                        onpointerdown: move |e| e.stop_propagation(),
                        button {
                            class: "{ICON_BUTTON} rounded-none",
                            title: "Hineinzoomen",
                            aria_label: "Hineinzoomen",
                            onclick: move |_| {
                                let (w, h) = *size.peek();
                                zoom_at(w / 2.0, h / 2.0, 2.0);
                            },
                            icons::Plus { class: "size-4" }
                        }
                        div { class: "h-px bg-border" }
                        button {
                            class: "{ICON_BUTTON} rounded-none",
                            title: "Herauszoomen",
                            aria_label: "Herauszoomen",
                            onclick: move |_| {
                                let (w, h) = *size.peek();
                                zoom_at(w / 2.0, h / 2.0, 0.5);
                            },
                            icons::Minus { class: "size-4" }
                        }
                    }

                    // Koordinaten
                    div { class: "pointer-events-none absolute bottom-3 left-3 z-30 flex items-center gap-2 rounded-md border bg-popover/90 px-2 py-1 font-mono text-xs shadow-sm",
                        if let Some((hx, hz)) = hover() {
                            span { "X {hx.floor()} · Z {hz.floor()}" }
                            span { class: "text-muted-foreground", "r.{(hx / REGION).floor()}.{(hz / REGION).floor()}" }
                        } else {
                            span { class: "text-muted-foreground", "X {x.floor()} · Z {z.floor()}" }
                        }
                        span { class: "text-muted-foreground", "· {fmt_scale(s)}" }
                    }
                }

                // Spielerliste
                div { class: "flex flex-col gap-2 rounded-xl border bg-card p-3 shadow-sm lg:max-h-[calc(100dvh-18rem)] lg:overflow-y-auto",
                    h2 { class: "px-1 text-xs font-medium uppercase tracking-wider text-muted-foreground", "Spieler" }
                    if plist.is_empty() {
                        p { class: "px-1 py-4 text-center text-sm text-muted-foreground", "Keine Spielerpositionen bekannt." }
                    }
                    for p in plist.iter().cloned() {
                        {
                            let here = same_dim(&p);
                            let pd = p.position.dimension.clone();
                            let (px, pz) = (p.position.x, p.position.z);
                            rsx! {
                                button {
                                    key: "{p.name}",
                                    class: "flex items-center gap-2.5 rounded-md px-2 py-1.5 text-left hover:bg-accent",
                                    title: "Auf Karte zentrieren",
                                    onclick: move |_| {
                                        if !here && !pd.is_empty() {
                                            dim.set(pd.clone());
                                            need_center.set(false);
                                        }
                                        center_on(px, pz);
                                    },
                                    div { class: "relative",
                                        PlayerHead { player: p.name.clone(), size: 7 }
                                        span { class: if p.online { "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-card bg-success" } else { "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full border-2 border-card bg-muted-foreground/50" } }
                                    }
                                    div { class: "min-w-0 flex-1",
                                        p { class: if p.online { "truncate text-sm font-medium" } else { "truncate text-sm text-muted-foreground" }, "{p.name}" }
                                        p { class: "truncate font-mono text-[11px] text-muted-foreground",
                                            "{px:.0} / {pz:.0}"
                                            if !here { " · {dimension_label(&pd)}" }
                                        }
                                    }
                                    icons::LocateFixed { class: "size-4 shrink-0 text-muted-foreground" }
                                }
                            }
                        }
                    }
                    if !plist.is_empty() {
                        Button {
                            size: ButtonSize::Sm,
                            variant: ButtonVariant::Ghost,
                            class: "mt-1 w-full",
                            onclick: {
                                let id = id.clone();
                                move |_| { nav.push(Route::ServerPlayers { id: id.clone() }); }
                            },
                            icons::Users { class: "size-4" }
                            "Alle Spieler"
                        }
                    }
                }
            }
            p { class: "text-xs text-muted-foreground",
                "Ziehen zum Verschieben, Mausrad oder zwei Finger zum Zoomen, Pfeiltasten und +/− mit der Tastatur. 1 Pixel = 1 Block bei 1×."
            }
        }
    }
}
