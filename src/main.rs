mod collector;
mod output;

use collector::Sample;
use std::collections::VecDeque;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

struct Config {
    interval: Duration,
    once: bool,
    json: bool,
    history: usize,
    history_file: Option<String>,
    alert_cpu: Option<f64>,
    alert_memory: Option<f64>,
    metrics_addr: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            once: false,
            json: false,
            history: 30,
            history_file: None,
            alert_cpu: None,
            alert_memory: None,
            metrics_addr: None,
        }
    }
}

const HELP: &str = r"mini-htop (Linux only)
Usage: mini-htop [OPTIONS]
  --interval SECONDS       Refresh interval (0.1 to 3600, default 1)
  --once                   Print one sample and exit
  --json                   Output one JSON object per sample
  --history N              Number of samples in charts (default 30)
  --history-file PATH      Append timestamped JSON samples to a file
  --alert-cpu PERCENT      Warn when CPU usage reaches this value
  --alert-memory PERCENT   Warn when memory usage reaches this value
  --metrics-addr HOST:PORT Serve Prometheus metrics at /metrics
  -h, --help               Show this help";

fn percent(value: Option<String>, option: &str) -> Result<f64, String> {
    let raw = value.ok_or_else(|| format!("{option} needs a value"))?;
    let value: f64 = raw.parse().map_err(|_| format!("invalid {option} value"))?;
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(format!("{option} must be between 0 and 100"));
    }
    Ok(value)
}

fn parse_args() -> Result<Option<Config>, String> {
    let mut config = Config::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--once" => config.once = true,
            "--json" => config.json = true,
            "--interval" => {
                let raw = args.next().ok_or("--interval needs a value")?;
                let seconds: f64 = raw.parse().map_err(|_| "invalid --interval value")?;
                if !seconds.is_finite() || !(0.1..=3600.0).contains(&seconds) {
                    return Err("--interval must be between 0.1 and 3600 seconds".into());
                }
                config.interval = Duration::from_secs_f64(seconds);
            }
            "--history" => {
                let raw = args.next().ok_or("--history needs a value")?;
                config.history = raw.parse().map_err(|_| "invalid --history value")?;
                if !(1..=1000).contains(&config.history) {
                    return Err("--history must be between 1 and 1000".into());
                }
            }
            "--history-file" => {
                config.history_file = Some(args.next().ok_or("--history-file needs a value")?)
            }
            "--alert-cpu" => config.alert_cpu = Some(percent(args.next(), &arg)?),
            "--alert-memory" => config.alert_memory = Some(percent(args.next(), &arg)?),
            "--metrics-addr" => {
                config.metrics_addr = Some(args.next().ok_or("--metrics-addr needs a value")?)
            }
            _ => return Err(format!("unknown option: {arg}")),
        }
    }
    if config.once && config.metrics_addr.is_some() {
        return Err("--once cannot be used with --metrics-addr".into());
    }
    Ok(Some(config))
}

fn alerts(sample: &Sample, config: &Config) -> Vec<String> {
    let mut result = Vec::new();
    if let Some(limit) = config.alert_cpu
        && sample.cpu_percent >= limit
    {
        result.push(format!("CPU {:.1}% >= {:.1}%", sample.cpu_percent, limit));
    }
    if let Some(limit) = config.alert_memory
        && sample.memory_percent >= limit
    {
        result.push(format!(
            "Memory {:.1}% >= {:.1}%",
            sample.memory_percent, limit
        ));
    }
    result
}

fn start_metrics(addr: &str, latest: Arc<RwLock<Option<Sample>>>) -> io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("metrics: http://{}/metrics", listener.local_addr()?);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let mut request = [0u8; 1024];
            let Ok(size) = stream.read(&mut request) else {
                continue;
            };
            let valid = request[..size].starts_with(b"GET /metrics HTTP/1.");
            let (status, body) = if valid {
                match latest.read() {
                    Ok(guard) => match guard.as_ref() {
                        Some(sample) => ("200 OK", output::prometheus(sample)),
                        None => ("503 Service Unavailable", "sample not ready\n".into()),
                    },
                    Err(_) => ("500 Internal Server Error", "metrics unavailable\n".into()),
                }
            } else {
                ("404 Not Found", "not found\n".into())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    Ok(())
}

fn remember(history: &mut VecDeque<f64>, value: f64, capacity: usize) {
    history.push_back(value);
    if history.len() > capacity {
        history.pop_front();
    }
}

fn run(config: Config) -> io::Result<()> {
    if !cfg!(target_os = "linux")
        || !Path::new("/proc/stat").exists()
        || !Path::new("/sys/block").exists()
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "live monitoring requires Linux with /proc and /sys mounted",
        ));
    }
    let latest = Arc::new(RwLock::new(None));
    if let Some(addr) = &config.metrics_addr {
        start_metrics(addr, Arc::clone(&latest))?;
    }
    let mut cpu_history = VecDeque::new();
    let mut memory_history = VecDeque::new();
    let mut history_file: Option<File> = config
        .history_file
        .as_ref()
        .map(|path| OpenOptions::new().create(true).append(true).open(path))
        .transpose()?;
    let mut before = collector::read()?;
    let clear = io::stdout().is_terminal() && !config.json && !config.once;
    loop {
        thread::sleep(config.interval);
        let now = collector::read()?;
        let sample = collector::calculate(&before, &now);
        let warnings = alerts(&sample, &config);
        remember(&mut cpu_history, sample.cpu_percent, config.history);
        remember(&mut memory_history, sample.memory_percent, config.history);
        if let Some(file) = &mut history_file {
            file.write_all(output::json_line(&sample, &warnings).as_bytes())?;
            file.flush()?;
        }
        if let Ok(mut guard) = latest.write() {
            *guard = Some(sample.clone());
        }
        if config.json {
            output::json(&sample, &warnings)?;
        } else {
            output::dashboard(&sample, &cpu_history, &memory_history, &warnings, clear)?;
        }
        if config.once {
            return Ok(());
        }
        before = now;
    }
}

fn main() {
    match parse_args() {
        Ok(None) => println!("{HELP}"),
        Err(message) => {
            eprintln!("{message}\n\n{HELP}");
            std::process::exit(2);
        }
        Ok(Some(config)) => {
            if let Err(error) = run(config) {
                eprintln!("mini-htop: {error}");
                std::process::exit(1);
            }
        }
    }
}
