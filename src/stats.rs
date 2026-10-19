//! Background sampler. Owns the sysinfo `System`, refreshes it on an adaptive interval and
//! hands the UI an immutable snapshot. The thread blocks on a channel between samples, so
//! while the window is hidden or paused it costs nothing.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, Signal, System, UpdateKind};

pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Percent of one core (can exceed 100 for multi-threaded processes).
    pub cpu: f32,
    pub mem: u64,
    /// Disk bytes per second.
    pub read: f32,
    pub write: f32,
    pub status: &'static str,
    pub age: u64,
}

#[derive(Default)]
pub struct Detail {
    pub pid: u32,
    pub exe: String,
    pub cmd: String,
}

#[derive(Default)]
pub struct Snapshot {
    pub seq: u64,
    pub procs: Vec<Proc>,
    pub cpu_total: f32,
    pub cores: Vec<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// Network bytes per second.
    pub net_rx: f32,
    pub net_tx: f32,
    pub disk_read: f32,
    pub disk_write: f32,
    pub uptime: u64,
    pub load: [f64; 3],
    pub cpu_name: String,
    pub detail: Option<Detail>,
    pub notice: Option<String>,
    /// How long the sampling itself took.
    pub cost: Duration,
    pub own_pid: u32,
}

pub enum Cmd {
    /// Sample period in ms; 0 pauses sampling entirely.
    Interval(u64),
    Watch(Option<u32>),
    /// Sample immediately (the window just came back into view).
    Now,
    Kill { pid: u32, force: bool },
}

pub fn spawn(ctx: eframe::egui::Context) -> (Sender<Cmd>, Receiver<Snapshot>) {
    let (cmd_tx, cmd_rx) = channel::<Cmd>();
    let (snap_tx, snap_rx) = channel::<Snapshot>();
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || run(ctx, cmd_rx, snap_tx))
        .expect("spawn sampler");
    (cmd_tx, snap_rx)
}

fn status_name(s: ProcessStatus) -> &'static str {
    match s {
        ProcessStatus::Run => "Running",
        ProcessStatus::Sleep | ProcessStatus::Idle => "Sleeping",
        ProcessStatus::Stop => "Stopped",
        ProcessStatus::Zombie => "Zombie",
        _ => "Other",
    }
}

fn run(ctx: eframe::egui::Context, cmds: Receiver<Cmd>, out: Sender<Snapshot>) {
    let mut sys = System::new();
    let mut nets = Networks::new_with_refreshed_list();
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory().with_disk_usage();
    let detail_kind = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet).with_cmd(UpdateKind::OnlyIfNotSet);
    let own = sysinfo::get_current_pid().map(|p| p.as_u32()).unwrap_or(0);
    let cpu_name = {
        sys.refresh_cpu_all();
        sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default()
    };
    // Prime CPU counters so the first real sample has a delta to work with.
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    nets.refresh(true);
    std::thread::sleep(Duration::from_millis(220));

    let mut interval: u64 = 1000;
    let mut watch: Option<u32> = None;
    let mut notice: Option<String> = None;
    let mut seq = 0u64;
    let mut last = Instant::now();

    loop {
        // sample
        let t0 = Instant::now();
        let dt = t0.duration_since(last).as_secs_f32().max(0.005);
        last = t0;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        nets.refresh(true);

        let mut detail = None;
        if let Some(pid) = watch {
            sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), false, detail_kind);
            if let Some(p) = sys.process(Pid::from_u32(pid)) {
                detail = Some(Detail {
                    pid,
                    exe: p.exe().map(|e| e.display().to_string()).unwrap_or_default(),
                    cmd: p.cmd().iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" "),
                });
            }
        }

        let (mut dr, mut dw) = (0.0f32, 0.0f32);
        let mut procs = Vec::with_capacity(sys.processes().len());
        for (pid, p) in sys.processes() {
            let du = p.disk_usage();
            let (r, w) = (du.read_bytes as f32 / dt, du.written_bytes as f32 / dt);
            dr += r;
            dw += w;
            procs.push(Proc {
                pid: pid.as_u32(),
                ppid: p.parent().map(|x| x.as_u32()).unwrap_or(0),
                name: p.name().to_string_lossy().into_owned(),
                cpu: p.cpu_usage(),
                mem: p.memory(),
                read: r,
                write: w,
                status: status_name(p.status()),
                age: p.run_time(),
            });
        }
        let (mut rx, mut tx) = (0u64, 0u64);
        for (name, n) in nets.iter() {
            if name.starts_with("lo") {
                continue;
            }
            rx += n.received();
            tx += n.transmitted();
        }
        let la = System::load_average();
        seq += 1;
        let snap = Snapshot {
            seq,
            procs,
            cpu_total: sys.global_cpu_usage(),
            cores: sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            mem_used: sys.used_memory(),
            mem_total: sys.total_memory(),
            swap_used: sys.used_swap(),
            swap_total: sys.total_swap(),
            net_rx: rx as f32 / dt,
            net_tx: tx as f32 / dt,
            disk_read: dr,
            disk_write: dw,
            uptime: System::uptime(),
            load: [la.one, la.five, la.fifteen],
            cpu_name: cpu_name.clone(),
            detail,
            notice: notice.take(),
            cost: t0.elapsed(),
            own_pid: own,
        };
        if out.send(snap).is_err() {
            return;
        }
        ctx.request_repaint();

        // wait until the next sample, or until a command arrives
        let mut deadline = Instant::now() + Duration::from_millis(interval.max(1));
        loop {
            let msg = if interval == 0 {
                cmds.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                cmds.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            };
            match msg {
                Ok(Cmd::Interval(ms)) => {
                    let was_paused = interval == 0;
                    interval = ms;
                    if ms == 0 {
                        continue;
                    }
                    if was_paused {
                        break; // resume with an immediate sample
                    }
                    deadline = Instant::now() + Duration::from_millis(ms);
                }
                Ok(Cmd::Watch(p)) => {
                    watch = p;
                    break;
                }
                Ok(Cmd::Now) => {
                    if interval > 0 {
                        break;
                    }
                }
                Ok(Cmd::Kill { pid, force }) => {
                    let name = sys.process(Pid::from_u32(pid)).map(|p| p.name().to_string_lossy().into_owned()).unwrap_or_default();
                    let ok = sys
                        .process(Pid::from_u32(pid))
                        .map(|p| if force { p.kill() } else { p.kill_with(Signal::Term).unwrap_or_else(|| p.kill()) })
                        .unwrap_or(false);
                    notice = Some(if ok {
                        format!("Sent {} to {name} ({pid})", if force { "force quit" } else { "quit" })
                    } else {
                        format!("Could not stop {name} ({pid}): permission denied or already gone")
                    });
                    break;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}
