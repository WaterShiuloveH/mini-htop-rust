use crate::collector::Sample;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{self, Write};

fn human(bytes: f64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut index = 0;
    while value >= 1024.0 && index < units.len() - 1 {
        value /= 1024.0;
        index += 1;
    }
    format!("{value:.1} {}", units[index])
}

fn chart(values: &VecDeque<f64>) -> String {
    const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values
        .iter()
        .map(|v| BARS[((v.clamp(0.0, 100.0) * 0.07).round() as usize).min(7)])
        .collect()
}

pub fn dashboard(
    sample: &Sample,
    cpu: &VecDeque<f64>,
    memory: &VecDeque<f64>,
    alerts: &[String],
    clear: bool,
) -> io::Result<()> {
    let mut output = String::new();
    if clear {
        output.push_str("\x1b[H\x1b[2J");
    }
    writeln!(output, "mini-htop  |  Ctrl-C to quit").unwrap();
    writeln!(
        output,
        "CPU     {:5.1}%  {}",
        sample.cpu_percent,
        chart(cpu)
    )
    .unwrap();
    writeln!(
        output,
        "Memory  {:5.1}%  {} / {}  {}",
        sample.memory_percent,
        human(sample.memory_used as f64),
        human(sample.memory_total as f64),
        chart(memory)
    )
    .unwrap();
    writeln!(
        output,
        "Disk    read {}/s  write {}/s",
        human(sample.disk_read_per_sec),
        human(sample.disk_write_per_sec)
    )
    .unwrap();
    writeln!(
        output,
        "Network RX   {}/s  TX    {}/s",
        human(sample.net_rx_per_sec),
        human(sample.net_tx_per_sec)
    )
    .unwrap();
    writeln!(output, "Processes: {}", sample.process_count).unwrap();
    for alert in alerts {
        writeln!(output, "ALERT   {alert}").unwrap();
    }
    writeln!(output, "\n{:>7} {:>7} {:>10}  NAME", "PID", "CPU%", "RSS").unwrap();
    for p in sample.processes.iter().take(12) {
        writeln!(
            output,
            "{:>7} {:>6.1}% {:>10}  {}",
            p.pid,
            p.cpu_percent,
            human(p.rss_bytes as f64),
            p.name.replace(['\n', '\r'], " ")
        )
        .unwrap();
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.flush()
}

fn escape(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            _ => out.push(c),
        }
    }
    out
}

pub fn json_line(sample: &Sample, alerts: &[String]) -> String {
    let mut output = format!(
        "{{\"timestamp_unix_ms\":{},\"cpu_percent\":{:.2},\"memory_percent\":{:.2},\"memory_used_bytes\":{},\"memory_total_bytes\":{},\"disk_read_bytes_per_sec\":{:.2},\"disk_write_bytes_per_sec\":{:.2},\"network_rx_bytes_per_sec\":{:.2},\"network_tx_bytes_per_sec\":{:.2},\"process_count\":{},\"processes\":[",
        sample.timestamp_unix_ms,
        sample.cpu_percent,
        sample.memory_percent,
        sample.memory_used,
        sample.memory_total,
        sample.disk_read_per_sec,
        sample.disk_write_per_sec,
        sample.net_rx_per_sec,
        sample.net_tx_per_sec,
        sample.process_count
    );
    for (index, p) in sample.processes.iter().take(12).enumerate() {
        if index > 0 {
            output.push(',');
        }
        write!(
            output,
            "{{\"pid\":{},\"name\":\"{}\",\"cpu_percent\":{:.2},\"rss_bytes\":{}}}",
            p.pid,
            escape(&p.name),
            p.cpu_percent,
            p.rss_bytes
        )
        .unwrap();
    }
    output.push_str("],\"alerts\":[");
    for (index, alert) in alerts.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        write!(output, "\"{}\"", escape(alert)).unwrap();
    }
    output.push_str("]}\n");
    output
}

pub fn json(sample: &Sample, alerts: &[String]) -> io::Result<()> {
    io::stdout().write_all(json_line(sample, alerts).as_bytes())
}

pub fn prometheus(sample: &Sample) -> String {
    format!(
        "# TYPE mini_htop_cpu_usage_percent gauge\nmini_htop_cpu_usage_percent {:.2}\n\
# TYPE mini_htop_memory_used_bytes gauge\nmini_htop_memory_used_bytes {}\n\
# TYPE mini_htop_memory_total_bytes gauge\nmini_htop_memory_total_bytes {}\n\
# TYPE mini_htop_disk_read_bytes_per_second gauge\nmini_htop_disk_read_bytes_per_second {:.2}\n\
# TYPE mini_htop_disk_write_bytes_per_second gauge\nmini_htop_disk_write_bytes_per_second {:.2}\n\
# TYPE mini_htop_network_rx_bytes_per_second gauge\nmini_htop_network_rx_bytes_per_second {:.2}\n\
# TYPE mini_htop_network_tx_bytes_per_second gauge\nmini_htop_network_tx_bytes_per_second {:.2}\n\
# TYPE mini_htop_process_count gauge\nmini_htop_process_count {}\n",
        sample.cpu_percent,
        sample.memory_used,
        sample.memory_total,
        sample.disk_read_per_sec,
        sample.disk_write_per_sec,
        sample.net_rx_per_sec,
        sample.net_tx_per_sec,
        sample.process_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_json_controls() {
        assert_eq!(escape("a\"\\\n\u{0001}"), "a\\\"\\\\\\n\\u0001");
    }
}
