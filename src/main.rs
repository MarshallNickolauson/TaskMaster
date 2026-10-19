#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod stats;
mod widgets;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind};
use stats::{Cmd, Proc, Snapshot};
use widgets::*;

const ROW_H: f32 = 24.0;
const BAR_H: f32 = 38.0;

fn main() -> eframe::Result {
    let mut vp = egui::ViewportBuilder::default()
        .with_inner_size([1240.0, 780.0])
        .with_min_inner_size([860.0, 520.0])
        .with_title("TaskMaster");
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        vp = vp.with_icon(icon);
    }
    eframe::run_native(
        "TaskMaster",
        eframe::NativeOptions { viewport: vp, ..Default::default() },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Processes,
    Performance,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Flat,
    Grouped,
    Tree,
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Name,
    Pid,
    Cpu,
    Mem,
    Read,
    Write,
    Age,
}

/// A visible table row. `idx` points into the snapshot's process list (or the group list).
#[derive(Clone, Copy)]
struct Row {
    idx: usize,
    depth: u16,
    group: bool,
    expandable: bool,
}

struct Group {
    name: String,
    members: Vec<usize>,
    cpu: f32,
    mem: u64,
    read: f32,
    write: f32,
    age: u64,
}

#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    mem: VecDeque<f32>,
    swap: VecDeque<f32>,
    rx: VecDeque<f32>,
    tx: VecDeque<f32>,
    dr: VecDeque<f32>,
    dw: VecDeque<f32>,
    sel_cpu: VecDeque<f32>,
    sel_mem: VecDeque<f32>,
}

struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Snapshot>,
    snap: Snapshot,
    hist: History,
    tab: Tab,
    mode: Mode,
    query: String,
    sort: (Col, bool),
    selected: Option<u32>,
    expanded_groups: HashSet<String>,
    expanded_pids: HashSet<u32>,
    groups: Vec<Group>,
    rows: Vec<Row>,
    dirty: bool,
    refresh_ms: u64,
    paused: bool,
    eco: bool,
    sent_interval: u64,
    was_idle: bool,
    confirm: Option<(u32, String)>,
    toast: Option<(String, Instant)>,
    /// Developer aid: `TASKMASTER_SHOT=out.png` saves a screenshot after a few samples, then quits.
    shot: Option<PathBuf>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut v = egui::Visuals::dark();
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(6, 9, 14);
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(TEXT);
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.window_corner_radius = egui::CornerRadius::ZERO;
        v.menu_corner_radius = egui::CornerRadius::ZERO;
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = egui::CornerRadius::ZERO;
        }
        v.widgets.inactive.bg_fill = SURFACE_2;
        v.widgets.inactive.weak_bg_fill = SURFACE_2;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE_2);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(30, 41, 58);
        v.widgets.hovered.bg_fill = Color32::from_rgb(30, 41, 58);
        cc.egui_ctx.set_visuals(v);
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 6.0);
            s.spacing.button_padding = vec2(12.0, 5.0);
        });
        let (tx, rx) = stats::spawn(cc.egui_ctx.clone());
        App {
            tx,
            rx,
            snap: Snapshot::default(),
            hist: History::default(),
            tab: Tab::Processes,
            mode: Mode::Flat,
            query: String::new(),
            sort: (Col::Cpu, true),
            selected: None,
            expanded_groups: HashSet::new(),
            expanded_pids: HashSet::new(),
            groups: Vec::new(),
            rows: Vec::new(),
            dirty: true,
            refresh_ms: 1000,
            paused: false,
            eco: true,
            sent_interval: 1000,
            was_idle: false,
            confirm: None,
            toast: None,
            shot: std::env::var_os("TASKMASTER_SHOT").map(PathBuf::from),
        }
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    /// Adaptive sampling: full speed when you are looking, slower when another window has focus,
    /// and fully stopped when minimized or paused.
    fn sync_interval(&mut self, ctx: &egui::Context) {
        let (focused, minimized) = ctx.input(|i| (i.viewport().focused.unwrap_or(true), i.viewport().minimized.unwrap_or(false)));
        let want = if self.paused || minimized {
            0
        } else if !focused && self.eco {
            (self.refresh_ms * 4).max(4000)
        } else {
            self.refresh_ms
        };
        if want != self.sent_interval {
            self.send(Cmd::Interval(want));
            self.sent_interval = want;
        }
        let idle = !focused || minimized;
        if self.was_idle && !idle {
            self.send(Cmd::Now);
        }
        self.was_idle = idle;
    }

    fn ingest(&mut self, s: Snapshot) {
        if s.seq > 1 {
            let h = &mut self.hist;
            push(&mut h.cpu, s.cpu_total);
            push(&mut h.mem, s.mem_used as f32 * 100.0 / s.mem_total.max(1) as f32);
            push(&mut h.swap, s.swap_used as f32 * 100.0 / s.swap_total.max(1) as f32);
            push(&mut h.rx, s.net_rx);
            push(&mut h.tx, s.net_tx);
            push(&mut h.dr, s.disk_read);
            push(&mut h.dw, s.disk_write);
            if let Some(p) = self.selected.and_then(|pid| s.procs.iter().find(|p| p.pid == pid)) {
                push(&mut h.sel_cpu, p.cpu);
                push(&mut h.sel_mem, p.mem as f32);
            }
        }
        if let Some(n) = &s.notice {
            self.toast = Some((n.clone(), Instant::now()));
        }
        self.snap = s;
        self.dirty = true;
    }

    fn select(&mut self, pid: Option<u32>) {
        if self.selected != pid {
            self.selected = pid;
            self.hist.sel_cpu.clear();
            self.hist.sel_mem.clear();
            self.send(Cmd::Watch(pid));
        }
    }

    fn matches(&self, p: &Proc, q: &str) -> bool {
        q.is_empty() || p.name.to_lowercase().contains(q) || p.pid.to_string() == q
    }

    fn cmp_proc(&self, a: &Proc, b: &Proc) -> std::cmp::Ordering {
        let (col, desc) = self.sort;
        let o = match col {
            Col::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            Col::Pid => a.pid.cmp(&b.pid),
            Col::Cpu => a.cpu.total_cmp(&b.cpu),
            Col::Mem => a.mem.cmp(&b.mem),
            Col::Read => a.read.total_cmp(&b.read),
            Col::Write => a.write.total_cmp(&b.write),
            Col::Age => a.age.cmp(&b.age),
        };
        if desc { o.reverse() } else { o }
    }

    fn rebuild(&mut self) {
        self.dirty = false;
        let q = self.query.trim().to_lowercase();
        let procs = &self.snap.procs;
        let mut rows = Vec::new();
        let mut groups: Vec<Group> = Vec::new();
        match self.mode {
            Mode::Flat => {
                let mut idx: Vec<usize> = (0..procs.len()).filter(|&i| self.matches(&procs[i], &q)).collect();
                idx.sort_by(|&a, &b| self.cmp_proc(&procs[a], &procs[b]));
                rows.extend(idx.into_iter().map(|idx| Row { idx, depth: 0, group: false, expandable: false }));
            }
            Mode::Grouped => {
                let mut by: HashMap<&str, Vec<usize>> = HashMap::new();
                for (i, p) in procs.iter().enumerate().filter(|(_, p)| self.matches(p, &q)) {
                    by.entry(p.name.as_str()).or_default().push(i);
                }
                for (name, mut members) in by {
                    members.sort_by(|&a, &b| self.cmp_proc(&procs[a], &procs[b]));
                    groups.push(Group {
                        name: name.to_string(),
                        cpu: members.iter().map(|&i| procs[i].cpu).sum(),
                        mem: members.iter().map(|&i| procs[i].mem).sum(),
                        read: members.iter().map(|&i| procs[i].read).sum(),
                        write: members.iter().map(|&i| procs[i].write).sum(),
                        age: members.iter().map(|&i| procs[i].age).max().unwrap_or(0),
                        members,
                    });
                }
                let (col, desc) = self.sort;
                groups.sort_by(|a, b| {
                    let o = match col {
                        Col::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                        Col::Pid => a.members.len().cmp(&b.members.len()),
                        Col::Cpu => a.cpu.total_cmp(&b.cpu),
                        Col::Mem => a.mem.cmp(&b.mem),
                        Col::Read => a.read.total_cmp(&b.read),
                        Col::Write => a.write.total_cmp(&b.write),
                        Col::Age => a.age.cmp(&b.age),
                    };
                    if desc { o.reverse() } else { o }
                });
                for (gi, g) in groups.iter().enumerate() {
                    let single = g.members.len() == 1;
                    if single {
                        rows.push(Row { idx: g.members[0], depth: 0, group: false, expandable: false });
                        continue;
                    }
                    rows.push(Row { idx: gi, depth: 0, group: true, expandable: true });
                    if self.expanded_groups.contains(&g.name) {
                        rows.extend(g.members.iter().map(|&idx| Row { idx, depth: 1, group: false, expandable: false }));
                    }
                }
            }
            Mode::Tree => {
                let alive: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
                let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
                let mut roots = Vec::new();
                for (i, p) in procs.iter().enumerate() {
                    if p.ppid != p.pid && alive.contains(&p.ppid) {
                        kids.entry(p.ppid).or_default().push(i);
                    } else {
                        roots.push(i);
                    }
                }
                if q.is_empty() {
                    roots.sort_by(|&a, &b| self.cmp_proc(&procs[a], &procs[b]));
                    for v in kids.values_mut() {
                        v.sort_by(|&a, &b| self.cmp_proc(&procs[a], &procs[b]));
                    }
                    let mut stack: Vec<(usize, u16)> = roots.into_iter().rev().map(|i| (i, 0)).collect();
                    while let Some((i, d)) = stack.pop() {
                        let pid = procs[i].pid;
                        let has = kids.contains_key(&pid);
                        rows.push(Row { idx: i, depth: d, group: false, expandable: has });
                        if has && self.expanded_pids.contains(&pid) {
                            stack.extend(kids[&pid].iter().rev().map(|&k| (k, d + 1)));
                        }
                    }
                } else {
                    let mut idx: Vec<usize> = (0..procs.len()).filter(|&i| self.matches(&procs[i], &q)).collect();
                    idx.sort_by(|&a, &b| self.cmp_proc(&procs[a], &procs[b]));
                    rows.extend(idx.into_iter().map(|idx| Row { idx, depth: 0, group: false, expandable: false }));
                }
            }
        }
        self.groups = groups;
        self.rows = rows;
    }

    fn move_selection(&mut self, delta: i32) {
        let pids: Vec<u32> = self.rows.iter().filter(|r| !r.group).map(|r| self.snap.procs[r.idx].pid).collect();
        if pids.is_empty() {
            return;
        }
        let cur = self.selected.and_then(|s| pids.iter().position(|&p| p == s));
        let next = match cur {
            Some(i) => (i as i32 + delta).clamp(0, pids.len() as i32 - 1) as usize,
            None => 0,
        };
        self.select(Some(pids[next]));
    }
}

/// A flat, square, uppercase-mono cell button for the top bar. Returns its click response.
fn cell(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let g = ui.painter().layout_no_wrap(text.to_uppercase(), FontId::monospace(11.0), if active { TEXT } else { MUTED });
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 28.0, BAR_H), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, SURFACE_2);
    }
    if active {
        p.rect_filled(Rect::from_min_size(pos2(rect.left(), rect.bottom() - 2.0), vec2(rect.width(), 2.0)), 0.0, ACCENT);
    }
    let col = if active { TEXT } else if resp.hovered() { TEXT } else { MUTED };
    p.galley(rect.center() - g.size() / 2.0, g, col);
    p.vline(rect.right(), rect.y_range(), Stroke::new(1.0, LINE));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        while let Ok(s) = self.rx.try_recv() {
            self.ingest(s);
        }
        self.sync_interval(&ctx);
        if let Some((_, t)) = &self.toast {
            if t.elapsed() > Duration::from_secs(6) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        if let Some(path) = &self.shot {
            if self.snap.seq >= 6 {
                let (path, c2) = (path.clone(), ctx.clone());
                self.shot = None;
                ctx.request_screenshot(move |img| {
                    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(&path, &raw, img.width() as u32, img.height() as u32, image::ColorType::Rgba8);
                    c2.send_viewport_cmd(egui::ViewportCommand::Close);
                });
            }
            ctx.request_repaint_after(Duration::from_millis(300));
        }

        // keyboard
        let search_id = egui::Id::unique("search");
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.tab = Tab::Processes;
            ctx.memory_mut(|m| m.request_focus(search_id));
        }
        let typing = ctx.egui_wants_keyboard_input();
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.query.clear();
            self.dirty = true;
            self.confirm = None;
            ctx.memory_mut(|m| m.surrender_focus(search_id));
        }
        if !typing && self.tab == Tab::Processes {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.move_selection(1);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.move_selection(-1);
            }
            if let Some(pid) = self.selected {
                if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
                    if let Some(p) = self.snap.procs.iter().find(|p| p.pid == pid) {
                        self.confirm = Some((pid, p.name.clone()));
                    }
                }
            }
        }

        // ---- top bar ----
        egui::Panel::top("bar")
            .exact_size(BAR_H)
            .frame(egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, LINE)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.horizontal_centered(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("TaskMaster").size(15.0).strong());
                    ui.add_space(14.0);
                    let (r, _) = ui.allocate_exact_size(vec2(1.0, BAR_H), Sense::hover());
                    ui.painter().vline(r.left(), r.y_range(), Stroke::new(1.0, LINE));
                    if cell(ui, "Processes", self.tab == Tab::Processes).clicked() {
                        self.tab = Tab::Processes;
                    }
                    if cell(ui, "Performance", self.tab == Tab::Performance).clicked() {
                        self.tab = Tab::Performance;
                    }
                    if self.tab == Tab::Processes {
                        for (m, n) in [(Mode::Flat, "Flat"), (Mode::Grouped, "Grouped"), (Mode::Tree, "Tree")] {
                            if cell(ui, n, self.mode == m).clicked() {
                                self.mode = m;
                                self.dirty = true;
                            }
                        }
                        ui.add_space(10.0);
                        let w = (ui.available_width() - 380.0).clamp(120.0, 340.0);
                        let r = ui.add_sized(
                            [w, 26.0],
                            egui::TextEdit::singleline(&mut self.query).id(search_id).hint_text("Search name or PID  (Ctrl/Cmd+F)").align(Align2::LEFT_CENTER),
                        );
                        if r.changed() {
                            self.dirty = true;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (dot, status) = if self.paused {
                            (FAINT, "Paused".to_string())
                        } else if self.sent_interval > self.refresh_ms {
                            (AMBER, format!("Eco {:.0}s", self.sent_interval as f32 / 1000.0))
                        } else {
                            (ACCENT, "Live".to_string())
                        };
                        ui.add_space(14.0);
                        ui.label(RichText::new(status.to_uppercase()).monospace().size(11.0).color(MUTED));
                        let (r, _) = ui.allocate_exact_size(vec2(14.0, BAR_H), Sense::hover());
                        ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(7.0, 7.0)), 0.0, dot);
                        ui.add_space(10.0);
                        let (r, _) = ui.allocate_exact_size(vec2(1.0, BAR_H), Sense::hover());
                        ui.painter().vline(r.left(), r.y_range(), Stroke::new(1.0, LINE));
                        if cell(ui, if self.paused { "Resume" } else { "Pause" }, self.paused).clicked() {
                            self.paused = !self.paused;
                        }
                        for (ms, n) in [(5000, "5s"), (2000, "2s"), (1000, "1s"), (500, "0.5s")] {
                            if cell(ui, n, self.refresh_ms == ms).clicked() {
                                self.refresh_ms = ms;
                            }
                        }
                        ui.label(RichText::new("RATE").monospace().size(10.0).color(FAINT));
                    });
                });
            });

        // ---- status bar ----
        egui::Panel::bottom("status")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(SURFACE).inner_margin(egui::Margin::symmetric(14, 0)).stroke(Stroke::new(1.0, LINE)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let mono = |t: String| RichText::new(t).monospace().size(11.0).color(MUTED);
                    let s = &self.snap;
                    ui.label(mono(format!("{} processes", s.procs.len())));
                    ui.label(mono(format!("up {}", fmt_age(s.uptime))));
                    ui.label(mono(format!("load {:.2} {:.2} {:.2}", s.load[0], s.load[1], s.load[2])));
                    if let Some((t, _)) = &self.toast {
                        ui.label(RichText::new(t).monospace().size(11.0).color(AMBER));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(me) = s.procs.iter().find(|p| p.pid == s.own_pid) {
                            ui.label(
                                RichText::new(format!("TaskMaster: {:.1}% CPU · {} · sample {:.1} ms", me.cpu, fmt_bytes(me.mem), s.cost.as_secs_f32() * 1000.0))
                                    .monospace()
                                    .size(11.0)
                                    .color(FAINT),
                            );
                        }
                    });
                });
            });

        // ---- summary strip ----
        egui::Panel::top("summary")
            .exact_size(74.0)
            .frame(egui::Frame::new().fill(BG).stroke(Stroke::new(1.0, LINE)))
            .show_separator_line(false)
            .show(ui, |ui| self.summary(ui));

        if self.dirty {
            self.rebuild();
        }

        match self.tab {
            Tab::Processes => {
                if let Some(pid) = self.selected {
                    egui::Panel::right("detail")
                        .default_size(330.0)
                        .size_range(260.0..=520.0)
                        .frame(egui::Frame::new().fill(SURFACE).stroke(Stroke::new(1.0, LINE)))
                        .show(ui, |ui| self.detail(ui, pid));
                }
                egui::CentralPanel::default().frame(egui::Frame::new().fill(BG)).show(ui, |ui| self.table(ui));
            }
            Tab::Performance => {
                egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(12))).show(ui, |ui| self.performance(ui));
            }
        }

        if let Some((pid, name)) = self.confirm.clone() {
            let mut close = false;
            egui::Window::new("End task")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(&ctx, |ui| {
                    ui.label(RichText::new(format!("End “{name}”?")).size(16.0).strong());
                    ui.label(RichText::new(format!("PID {pid}. Unsaved work in it may be lost.")).color(MUTED));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("End task").color(Color32::BLACK)).fill(ACCENT)).clicked() {
                            self.send(Cmd::Kill { pid, force: false });
                            close = true;
                        }
                        if ui.add(egui::Button::new(RichText::new("Force quit").color(Color32::WHITE)).fill(RED.gamma_multiply(0.7))).clicked() {
                            self.send(Cmd::Kill { pid, force: true });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if close {
                self.confirm = None;
            }
        }
    }
}

impl App {
    fn summary(&mut self, ui: &mut egui::Ui) {
        let s = &self.snap;
        let h = &self.hist;
        let rect = ui.available_rect_before_wrap();
        let p = ui.painter_at(rect);
        let n = 5.0;
        let cw = rect.width() / n;
        let net_max = h.rx.iter().chain(h.tx.iter()).copied().fold(0.0f32, f32::max);
        let dsk_max = h.dr.iter().chain(h.dw.iter()).copied().fold(0.0f32, f32::max);
        let _ = (net_max, dsk_max);
        let mem_pct = s.mem_used as f32 * 100.0 / s.mem_total.max(1) as f32;
        let cells: [(&str, String, String, Vec<(&VecDeque<f32>, Color32)>, Option<f32>); 5] = [
            ("CPU", format!("{:.0}%", s.cpu_total), format!("{} cores", s.cores.len()), vec![(&h.cpu, heat(s.cpu_total / 100.0))], Some(100.0)),
            ("Memory", format!("{:.0}%", mem_pct), format!("{} of {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)), vec![(&h.mem, heat(mem_pct / 100.0))], Some(100.0)),
            ("Swap", fmt_bytes(s.swap_used), format!("of {}", fmt_bytes(s.swap_total)), vec![(&h.swap, VIOLET)], Some(100.0)),
            ("Network", format!("In {}", fmt_rate(s.net_rx)), format!("Out {}", fmt_rate(s.net_tx)), vec![(&h.rx, CYAN), (&h.tx, AMBER)], None),
            ("Disk", format!("R {}", fmt_rate(s.disk_read)), format!("W {}", fmt_rate(s.disk_write)), vec![(&h.dr, ACCENT), (&h.dw, RED)], None),
        ];
        for (i, (title, big, small, series, max)) in cells.iter().enumerate() {
            let r = Rect::from_min_size(rect.min + vec2(cw * i as f32, 0.0), vec2(cw, rect.height()));
            caps(&p, r.left_top() + vec2(14.0, 11.0), Align2::LEFT_TOP, title, FAINT);
            p.text(r.left_top() + vec2(14.0, 26.0), Align2::LEFT_TOP, big, FontId::proportional(19.0), TEXT);
            p.text(r.left_top() + vec2(14.0, 52.0), Align2::LEFT_TOP, small, FontId::monospace(10.5), MUTED);
            let spark = Rect::from_min_max(pos2(r.left() + cw * 0.5, r.top() + 10.0), pos2(r.right() - 12.0, r.bottom() - 10.0));
            sparkline(&p, spark, series, *max);
            if i > 0 {
                p.vline(r.left(), r.y_range(), Stroke::new(1.0, LINE));
            }
        }
        ui.allocate_rect(rect, Sense::hover());
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        // column layout, right to left
        let cols: [(Col, &str, f32); 6] = [
            (Col::Age, "Uptime", 78.0),
            (Col::Write, "Disk W", 84.0),
            (Col::Read, "Disk R", 84.0),
            (Col::Mem, "Memory", 96.0),
            (Col::Cpu, "CPU %", 92.0),
            (Col::Pid, "PID", 66.0),
        ];
        let total_w: f32 = cols.iter().map(|c| c.2).sum::<f32>() + 14.0;

        // header
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        {
            let p = ui.painter_at(hr);
            p.rect_filled(hr, 0.0, SURFACE);
            p.hline(hr.x_range(), hr.bottom(), Stroke::new(1.0, LINE));
        }
        let mut x = hr.right() - 14.0;
        let header_cell = |ui: &mut egui::Ui, col: Col, text: &str, rect: Rect, right: bool, this: &mut Self| {
            let resp = ui.interact(rect, ui.scope_id().with(("h", text)), Sense::click());
            let arrow = if this.sort.0 == col { if this.sort.1 { " v" } else { " ^" } } else { "" };
            let color = if this.sort.0 == col { TEXT } else if resp.hovered() { TEXT } else { FAINT };
            let (pos, anchor) = if right { (pos2(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER) } else { (pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER) };
            ui.painter().text(pos, anchor, format!("{}{arrow}", text.to_uppercase()), FontId::monospace(10.5), color);
            if resp.clicked() {
                this.sort = if this.sort.0 == col { (col, !this.sort.1) } else { (col, !matches!(col, Col::Name | Col::Pid)) };
                this.dirty = true;
            }
        };
        for (col, t, w) in cols {
            let r = Rect::from_min_max(pos2(x - w, hr.top()), pos2(x, hr.bottom()));
            header_cell(ui, col, t, r, true, self);
            x -= w;
        }
        let nr = Rect::from_min_max(hr.left_top(), pos2(x, hr.bottom()));
        header_cell(ui, Col::Name, "Name", nr, false, self);

        let mem_total = self.snap.mem_total.max(1) as f32;
        let ncores = self.snap.cores.len().max(1) as f32;
        let mut clicked: Option<Option<u32>> = None;
        let mut toggle_group: Option<String> = None;
        let mut toggle_pid: Option<u32> = None;
        let mut ask_kill: Option<(u32, String)> = None;
        let rows = &self.rows;
        let procs = &self.snap.procs;
        let groups = &self.groups;
        let selected = self.selected;
        let (eg, ep) = (&self.expanded_groups, &self.expanded_pids);
        let mode = self.mode;

        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, ROW_H, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let p = ui.painter_at(rect);
                // pull the numbers for either a real process or a group aggregate
                let (name, pid, cpu, mem, rd, wr, age, id_sel): (&str, Option<u32>, f32, u64, f32, f32, u64, Option<u32>) = if row.group {
                    let g = &groups[row.idx];
                    (&g.name, None, g.cpu, g.mem, g.read, g.write, g.age, None)
                } else {
                    let q = &procs[row.idx];
                    (&q.name, Some(q.pid), q.cpu, q.mem, q.read, q.write, q.age, Some(q.pid))
                };
                let is_sel = id_sel.is_some() && id_sel == selected;
                if is_sel {
                    p.rect_filled(rect, 0.0, ACCENT.gamma_multiply(0.16));
                    p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, ROW_H)), 0.0, ACCENT);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, Color32::from_white_alpha(8));
                } else if row.group {
                    p.rect_filled(rect, 0.0, SURFACE.gamma_multiply(0.6));
                }
                p.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, LINE.gamma_multiply(0.45)));

                let mid = rect.center().y;
                let mut x = rect.right() - 14.0;
                let num = |p: &egui::Painter, x: f32, text: String, color: Color32| {
                    p.text(pos2(x - 6.0, mid), Align2::RIGHT_CENTER, text, FontId::proportional(12.5), color);
                };
                // Uptime
                num(&p, x, fmt_age(age), FAINT);
                x -= 78.0;
                num(&p, x, fmt_rate(wr), if wr > 1024.0 { TEXT } else { FAINT });
                x -= 84.0;
                num(&p, x, fmt_rate(rd), if rd > 1024.0 { TEXT } else { FAINT });
                x -= 84.0;
                // Memory with a share-of-RAM bar
                let mfrac = (mem as f32 / mem_total).clamp(0.0, 1.0);
                p.rect_filled(Rect::from_min_max(pos2(x - 96.0 + 2.0, rect.bottom() - 4.0), pos2(x - 96.0 + 2.0 + 92.0 * (mfrac * 4.0).min(1.0), rect.bottom() - 2.0)), 0.0, VIOLET.gamma_multiply(0.7));
                num(&p, x, fmt_bytes(mem), TEXT);
                x -= 96.0;
                // CPU with heat tint
                let cfrac = (cpu / 100.0).clamp(0.0, 1.0);
                let tint = heat((cpu / (100.0 * ncores.min(4.0))).clamp(0.0, 1.0));
                if cpu >= 0.1 {
                    p.rect_filled(Rect::from_min_max(pos2(x - 92.0, rect.top() + 1.0), pos2(x, rect.bottom() - 1.0)), 0.0, tint.gamma_multiply(0.06 + 0.3 * cfrac.max((cpu / 100.0 / ncores).min(1.0))));
                    p.rect_filled(Rect::from_min_max(pos2(x - 92.0 + 2.0, rect.bottom() - 4.0), pos2(x - 92.0 + 2.0 + 88.0 * cfrac, rect.bottom() - 2.0)), 0.0, tint);
                }
                num(&p, x, format!("{cpu:.1}"), if cpu >= 50.0 { tint } else { TEXT });
                x -= 92.0;
                // PID (or process count for a group)
                let pid_txt = match pid {
                    Some(pid) => pid.to_string(),
                    None => format!("×{}", groups[row.idx].members.len()),
                };
                num(&p, x, pid_txt, FAINT);
                x -= 66.0;

                // name cell: indent, chevron, tile, text
                let mut nx = rect.left() + 12.0 + row.depth as f32 * 16.0;
                if row.expandable {
                    let open = if row.group { eg.contains(name) } else { pid.is_some_and(|q| ep.contains(&q)) };
                    let c = pos2(nx + 4.0, mid);
                    let pts = if open { vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)] } else { vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)] };
                    p.add(egui::Shape::convex_polygon(pts, MUTED, Stroke::NONE));
                }
                if mode != Mode::Flat || row.expandable {
                    nx += 16.0;
                }
                let tile = Rect::from_center_size(pos2(nx + 7.0, mid), vec2(14.0, 14.0));
                p.rect_filled(tile, 0.0, name_color(name).gamma_multiply(0.9));
                p.text(tile.center(), Align2::CENTER_CENTER, name.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('?').to_string(), FontId::monospace(10.0), Color32::from_rgb(8, 12, 20));
                label(&p, Rect::from_min_max(pos2(nx + 22.0, rect.top()), pos2(x - 4.0, rect.bottom())), name, FontId::proportional(13.0), if row.group { TEXT } else { TEXT.gamma_multiply(0.92) });

                if resp.clicked() {
                    if row.expandable && resp.interact_pointer_pos().is_some_and(|ip| ip.x < nx + 12.0) {
                        if row.group {
                            toggle_group = Some(name.to_string());
                        } else {
                            toggle_pid = pid;
                        }
                    } else if row.group {
                        toggle_group = Some(name.to_string());
                    } else {
                        clicked = Some(pid);
                    }
                }
                if resp.double_clicked() && row.expandable && !row.group {
                    toggle_pid = pid;
                }
                if let Some(pid) = pid {
                    resp.context_menu(|ui| {
                        if ui.button("End task…").clicked() {
                            ask_kill = Some((pid, name.to_string()));
                            ui.close();
                        }
                        if ui.button("Copy PID").clicked() {
                            ui.ctx().copy_text(pid.to_string());
                            ui.close();
                        }
                        if ui.button("Copy name").clicked() {
                            ui.ctx().copy_text(name.to_string());
                            ui.close();
                        }
                    });
                }
            }
        });

        let _ = total_w;
        if let Some(c) = clicked {
            self.select(c);
        }
        if let Some(g) = toggle_group {
            if !self.expanded_groups.remove(&g) {
                self.expanded_groups.insert(g);
            }
            self.dirty = true;
        }
        if let Some(pid) = toggle_pid {
            if !self.expanded_pids.remove(&pid) {
                self.expanded_pids.insert(pid);
            }
            self.select(Some(pid));
            self.dirty = true;
        }
        if ask_kill.is_some() {
            self.confirm = ask_kill;
        }
        if self.rows.is_empty() {
            ui.painter().text(ui.max_rect().center(), Align2::CENTER_CENTER, if self.snap.seq == 0 { "Reading processes…" } else { "No matching processes" }, FontId::proportional(14.0), FAINT);
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, pid: u32) {
        let procs = &self.snap.procs;
        let Some(p) = procs.iter().find(|p| p.pid == pid) else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("This process has exited.").color(MUTED));
                if ui.button("Close").clicked() {
                    self.select(None);
                }
            });
            return;
        };
        let parent = procs.iter().find(|q| q.pid == p.ppid).map(|q| format!("{} ({})", q.name, q.pid)).unwrap_or_else(|| p.ppid.to_string());
        let ppid = p.ppid;
        let (name, status, age, cpu, mem) = (p.name.clone(), p.status, p.age, p.cpu, p.mem);
        let (rd, wr) = (p.read, p.write);
        let detail = self.snap.detail.as_ref().filter(|d| d.pid == pid);
        let (exe, cmd) = detail.map(|d| (d.exe.clone(), d.cmd.clone())).unwrap_or_default();
        let mem_total = self.snap.mem_total;

        egui::Frame::new().inner_margin(egui::Margin::same(14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                ui.painter().rect_filled(r, 0.0, name_color(&name));
                ui.painter().text(r.center(), Align2::CENTER_CENTER, name.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('?').to_string(), FontId::monospace(15.0), Color32::from_rgb(8, 12, 20));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&name).size(16.0).strong());
                    ui.label(RichText::new(format!("PID {pid} · {status}")).monospace().size(11.0).color(MUTED));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if ui.small_button("✕").clicked() {
                        self.select(None);
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let can = pid > 1;
                if ui.add_enabled(can, egui::Button::new(RichText::new("End task").color(Color32::BLACK)).fill(ACCENT)).clicked() {
                    self.confirm = Some((pid, name.clone()));
                }
                if ui.add_enabled(can, egui::Button::new("Force quit")).clicked() {
                    self.send(Cmd::Kill { pid, force: true });
                }
            });
            ui.add_space(12.0);

            let w = ui.available_width();
            graph(ui, vec2(w, 110.0), "CPU", &format!("{cpu:.1}%"), &[(&self.hist.sel_cpu, heat(cpu / 100.0))], None, &|v| format!("{v:.0}%"));
            ui.add_space(8.0);
            graph(ui, vec2(w, 110.0), "Memory", &fmt_bytes(mem), &[(&self.hist.sel_mem, VIOLET)], None, &|v| fmt_bytes(v as u64));
            ui.add_space(12.0);

            let kv = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.horizontal_top(|ui| {
                    ui.add_sized([78.0, 16.0], egui::Label::new(RichText::new(k.to_uppercase()).monospace().size(10.5).color(FAINT)));
                    ui.add(egui::Label::new(RichText::new(v).size(12.5)).wrap());
                });
            };
            kv(ui, "Share", format!("{:.2}% of RAM", mem as f32 * 100.0 / mem_total.max(1) as f32));
            kv(ui, "Disk", format!("R {}  ·  W {}", fmt_rate(rd), fmt_rate(wr)));
            kv(ui, "Uptime", fmt_age(age));
            ui.horizontal_top(|ui| {
                ui.add_sized([78.0, 16.0], egui::Label::new(RichText::new("PARENT").monospace().size(10.5).color(FAINT)));
                if ui.add(egui::Label::new(RichText::new(&parent).size(12.5).color(ACCENT)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && ppid > 0 {
                    self.select(Some(ppid));
                }
            });
            if !exe.is_empty() {
                kv(ui, "Path", exe);
            }
            if !cmd.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("COMMAND").monospace().size(10.5).color(FAINT));
                egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(cmd).monospace().size(11.0).color(MUTED)).wrap());
                });
            }
        });
    }

    fn performance(&mut self, ui: &mut egui::Ui) {
        let s = &self.snap;
        let h = &self.hist;
        let w = ui.available_width();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            let cpu_title = if s.cpu_name.is_empty() { "CPU".to_string() } else { format!("CPU · {}", s.cpu_name) };
            graph(ui, vec2(w, 210.0), &cpu_title, &format!("{:.0}%", s.cpu_total), &[(&h.cpu, heat(s.cpu_total / 100.0))], Some(100.0), &|v| format!("{v:.0}%"));

            // per-core bars
            let n = s.cores.len().max(1);
            let cols = n.min(16);
            let rowsn = n.div_ceil(cols);
            let (rect, _) = ui.allocate_exact_size(vec2(w, 40.0 + rowsn as f32 * 54.0), Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 0.0, SURFACE);
            p.rect_stroke(rect, 0.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
            caps(&p, rect.left_top() + vec2(14.0, 14.0), Align2::LEFT_TOP, "Cores", MUTED);
            let cw = (rect.width() - 28.0) / cols as f32;
            for (i, c) in s.cores.iter().enumerate() {
                let (cx, cy) = ((i % cols) as f32, (i / cols) as f32);
                let r = Rect::from_min_size(pos2(rect.left() + 14.0 + cx * cw + 3.0, rect.top() + 38.0 + cy * 54.0), vec2(cw - 6.0, 40.0));
                p.rect_filled(r, 0.0, SURFACE_2);
                let fh = r.height() * (c / 100.0).clamp(0.0, 1.0);
                p.rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - fh), r.right_bottom()), 0.0, heat(c / 100.0));
                p.text(pos2(r.center().x, r.bottom() + 8.0), Align2::CENTER_CENTER, format!("{}", i + 1), FontId::monospace(9.5), FAINT);
            }

            let tw = (w - 20.0) / 3.0;
            ui.horizontal(|ui| {
                graph(ui, vec2(tw, 190.0), "Memory", &format!("{} / {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total)), &[(&h.mem, VIOLET)], Some(100.0), &|v| format!("{v:.0}%"));
                graph(ui, vec2(tw, 190.0), "Network", &format!("In {}  Out {}", fmt_rate(s.net_rx), fmt_rate(s.net_tx)), &[(&h.rx, CYAN), (&h.tx, AMBER)], None, &|v| fmt_rate(v));
                graph(ui, vec2(tw, 190.0), "Disk I/O", &format!("R {}  W {}", fmt_rate(s.disk_read), fmt_rate(s.disk_write)), &[(&h.dr, ACCENT), (&h.dw, RED)], None, &|v| fmt_rate(v));
            });
            ui.label(RichText::new(format!("Swap {} of {}  ·  history covers the last {} samples", fmt_bytes(s.swap_used), fmt_bytes(s.swap_total), HISTORY)).monospace().size(11.0).color(FAINT));
        });
    }
}
