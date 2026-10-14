use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    ticks: u64,
    start: u64,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub timestamp_unix_ms: u128,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_read_per_sec: f64,
    pub disk_write_per_sec: f64,
    pub net_rx_per_sec: f64,
    pub net_tx_per_sec: f64,
    pub process_count: usize,
    pub processes: Vec<Process>,
}

#[derive(Default)]
struct Cpu {
    total: u64,
    idle: u64,
    cores: usize,
}

pub struct Raw {
    cpu: Cpu,
    mem_total: u64,
    mem_available: u64,
    disk_read: u64,
    disk_write: u64,
    net_rx: u64,
    net_tx: u64,
    processes: Vec<Process>,
    at: Instant,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn cpu(input: &str) -> io::Result<Cpu> {
    let mut lines = input.lines();
    let first = lines.next().ok_or_else(|| invalid("missing CPU line"))?;
    let mut fields = first.split_whitespace();
    if fields.next() != Some("cpu") {
        return Err(invalid("invalid CPU line"));
    }
    let ticks: Vec<u64> = fields
        .take(8)
        .map(|v| v.parse().map_err(|_| invalid("invalid CPU counter")))
        .collect::<io::Result<_>>()?;
    if ticks.len() < 4 {
        return Err(invalid("incomplete CPU counters"));
    }
    let cores = lines
        .filter(|line| {
            line.split_whitespace()
                .next()
                .and_then(|v| v.strip_prefix("cpu"))
                .is_some_and(|v| v.parse::<usize>().is_ok())
        })
        .count()
        .max(1);
    Ok(Cpu {
        total: ticks.iter().sum(),
        idle: ticks[3] + ticks.get(4).copied().unwrap_or(0),
        cores,
    })
}

fn memory(input: &str) -> io::Result<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key == "MemTotal" || key == "MemAvailable" {
            let kib: u64 = value
                .split_whitespace()
                .next()
                .ok_or_else(|| invalid("missing memory value"))?
                .parse()
                .map_err(|_| invalid("invalid memory value"))?;
            if key == "MemTotal" {
                total = Some(kib.saturating_mul(1024));
            } else {
                available = Some(kib.saturating_mul(1024));
            }
        }
    }
    Ok((
        total.ok_or_else(|| invalid("missing MemTotal"))?,
        available.ok_or_else(|| invalid("missing MemAvailable"))?,
    ))
}

fn disk(input: &str, devices: &HashSet<String>) -> (u64, u64) {
    let mut read = 0u64;
    let mut write = 0u64;
    for line in input.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() > 9 && devices.contains(f[2]) {
            read = read.saturating_add(f[5].parse::<u64>().unwrap_or(0).saturating_mul(512));
            write = write.saturating_add(f[9].parse::<u64>().unwrap_or(0).saturating_mul(512));
        }
    }
    (read, write)
}

fn network(input: &str) -> (u64, u64) {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in input.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<_> = counters.split_whitespace().collect();
        if f.len() >= 16 {
            rx = rx.saturating_add(f[0].parse::<u64>().unwrap_or(0));
            tx = tx.saturating_add(f[8].parse::<u64>().unwrap_or(0));
        }
    }
    (rx, tx)
}

fn process(pid: u32, input: &str, page_size: u64) -> io::Result<Process> {
    let open = input
        .find('(')
        .ok_or_else(|| invalid("missing process name"))?;
    let close = input
        .rfind(") ")
        .ok_or_else(|| invalid("missing process fields"))?;
    let f: Vec<_> = input[close + 2..].split_whitespace().collect();
    if f.len() < 22 {
        return Err(invalid("incomplete process fields"));
    }
    let number = |i: usize| -> io::Result<u64> {
        f[i].parse().map_err(|_| invalid("invalid process counter"))
    };
    Ok(Process {
        pid,
        name: input[open + 1..close].to_owned(),
        cpu_percent: 0.0,
        rss_bytes: number(21)?.saturating_mul(page_size),
        ticks: number(11)?.saturating_add(number(12)?),
        start: number(19)?,
    })
}

unsafe extern "C" {
    fn getpagesize() -> std::ffi::c_int;
}

pub fn read() -> io::Result<Raw> {
    let cpu = cpu(&fs::read_to_string("/proc/stat")?)?;
    let (mem_total, mem_available) = memory(&fs::read_to_string("/proc/meminfo")?)?;
    let block_entries: Vec<_> = fs::read_dir("/sys/block")?.filter_map(Result::ok).collect();
    let slaves: HashSet<String> = block_entries
        .iter()
        .flat_map(|entry| {
            fs::read_dir(entry.path().join("slaves"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|slave| slave.file_name().to_string_lossy().into_owned())
        })
        .collect();
    let devices: HashSet<String> = block_entries
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            !name.starts_with("loop") && !name.starts_with("ram") && !slaves.contains(name)
        })
        .collect();
    let (disk_read, disk_write) = disk(&fs::read_to_string("/proc/diskstats")?, &devices);
    let (net_rx, net_tx) = network(&fs::read_to_string("/proc/net/dev")?);
    let page_size = unsafe { getpagesize() } as u64;
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let Ok(entry) = entry else { continue };
        let Ok(pid) = entry.file_name().to_string_lossy().parse() else {
            continue;
        };
        if let Ok(stat) = fs::read_to_string(entry.path().join("stat"))
            && let Ok(p) = process(pid, &stat, page_size)
        {
            processes.push(p);
        }
    }
    Ok(Raw {
        cpu,
        mem_total,
        mem_available,
        disk_read,
        disk_write,
        net_rx,
        net_tx,
        processes,
        at: Instant::now(),
    })
}

pub fn calculate(before: &Raw, now: &Raw) -> Sample {
    let seconds = now.at.duration_since(before.at).as_secs_f64().max(0.001);
    let ticks = now.cpu.total.saturating_sub(before.cpu.total);
    let idle = now.cpu.idle.saturating_sub(before.cpu.idle);
    let cpu_percent = if ticks == 0 {
        0.0
    } else {
        100.0 * ticks.saturating_sub(idle) as f64 / ticks as f64
    };
    let memory_used = now.mem_total.saturating_sub(now.mem_available);
    let memory_percent = if now.mem_total == 0 {
        0.0
    } else {
        100.0 * memory_used as f64 / now.mem_total as f64
    };
    let previous: HashMap<u32, (u64, u64)> = before
        .processes
        .iter()
        .map(|p| (p.pid, (p.start, p.ticks)))
        .collect();
    let mut processes = now.processes.clone();
    for p in &mut processes {
        if let Some((start, old)) = previous.get(&p.pid)
            && *start == p.start
            && ticks > 0
        {
            p.cpu_percent =
                100.0 * p.ticks.saturating_sub(*old) as f64 * now.cpu.cores as f64 / ticks as f64;
        }
    }
    processes.sort_by(|a, b| {
        b.cpu_percent
            .total_cmp(&a.cpu_percent)
            .then_with(|| b.rss_bytes.cmp(&a.rss_bytes))
    });
    let rate = |current: u64, old: u64| current.saturating_sub(old) as f64 / seconds;
    Sample {
        timestamp_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        cpu_percent,
        memory_percent,
        memory_used,
        memory_total: now.mem_total,
        disk_read_per_sec: rate(now.disk_read, before.disk_read),
        disk_write_per_sec: rate(now.disk_write, before.disk_write),
        net_rx_per_sec: rate(now.net_rx, before.net_rx),
        net_tx_per_sec: rate(now.net_tx, before.net_tx),
        process_count: processes.len(),
        processes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_system_counters() {
        let c = cpu("cpu  10 0 5 80 5 0 0 0 0 0\ncpu0 1 0 0 0\ncpu1 1 0 0 0\n").unwrap();
        assert_eq!((c.total, c.idle, c.cores), (100, 85, 2));
        assert_eq!(
            memory("MemTotal: 1000 kB\nMemAvailable: 250 kB\n").unwrap(),
            (1_024_000, 256_000)
        );
        let devices = ["sda".to_string()].into_iter().collect();
        assert_eq!(
            disk(
                "8 0 sda 1 0 10 0 2 0 20 0\n8 1 sda1 1 0 99 0 2 0 99 0",
                &devices
            ),
            (5120, 10240)
        );
        let net = "Inter-| Receive | Transmit\n face |bytes|bytes\n lo: 99 0 0 0 0 0 0 0 99 0 0 0 0 0 0 0\neth0: 10 0 0 0 0 0 0 0 20 0 0 0 0 0 0 0\n";
        assert_eq!(network(net), (10, 20));
    }

    #[test]
    fn process_name_can_contain_parentheses() {
        let f = "S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22";
        let p = process(42, &format!("42 (worker (io)) {f}"), 4096).unwrap();
        assert_eq!(p.name, "worker (io)");
        assert_eq!(p.ticks, 23);
        assert_eq!(p.start, 19);
        assert_eq!(p.rss_bytes, 21 * 4096);
    }

    #[test]
    fn calculates_rates_and_ignores_reused_pid() {
        let at = Instant::now();
        let previous = Process {
            pid: 42,
            name: "old".into(),
            cpu_percent: 0.0,
            rss_bytes: 100,
            ticks: 50,
            start: 1,
        };
        let before = Raw {
            cpu: Cpu {
                total: 100,
                idle: 50,
                cores: 2,
            },
            mem_total: 1000,
            mem_available: 750,
            disk_read: 100,
            disk_write: 200,
            net_rx: 300,
            net_tx: 400,
            processes: vec![previous],
            at,
        };
        let current = Process {
            pid: 42,
            name: "new".into(),
            cpu_percent: 0.0,
            rss_bytes: 200,
            ticks: 100,
            start: 2,
        };
        let now = Raw {
            cpu: Cpu {
                total: 300,
                idle: 150,
                cores: 2,
            },
            mem_total: 1000,
            mem_available: 500,
            disk_read: 300,
            disk_write: 400,
            net_rx: 500,
            net_tx: 600,
            processes: vec![current],
            at: at + std::time::Duration::from_secs(2),
        };
        let sample = calculate(&before, &now);
        assert_eq!(sample.cpu_percent, 50.0);
        assert_eq!(sample.memory_percent, 50.0);
        assert_eq!(sample.disk_read_per_sec, 100.0);
        assert_eq!(sample.net_tx_per_sec, 100.0);
        assert_eq!(sample.processes[0].cpu_percent, 0.0);
    }
}
