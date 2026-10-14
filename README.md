# mini-htop

A small Linux system monitor written in Rust using only the standard library. It shows CPU, memory, disk I/O, network traffic, and processes sorted by CPU usage. It also supports recent history charts, JSONL history files, threshold alerts, JSON output, and a Prometheus `/metrics` endpoint.

## Run on macOS with Docker

mini-htop needs Linux `/proc` and `/sys`. Run every command in this section from a **macOS terminal**; `docker run` starts a Linux container for the program. Running `cargo run` directly on macOS cannot collect live statistics. Docker downloads the `rust:1.94-alpine` image on first use.

### 1. Prepare and test

Start Docker Desktop or Rancher Desktop and wait for the container engine to be ready. Go to the project directory and check that Docker is using Linux:

```sh
cd ~/Desktop/mini-htop
docker info --format '{{.OSType}}'
```

The second command should print `linux`. Then run the tests and take one sample inside Linux containers:

```sh
docker run --rm -v "$PWD":/work -w /work rust:1.94-alpine cargo test
docker run --rm -v "$PWD":/work -w /work rust:1.94-alpine cargo run -- --once --json --interval 0.1
```

The tests should report `4 passed`, and the sample command should print one JSON line. `--once` exits after that sample, so it does not keep an HTTP server running.

### 2. Run the dashboard and HTTP server

In **macOS terminal A**, while still in the project directory, run the command below. `-it` keeps the dashboard in this terminal, `--init` lets `Ctrl-C` stop the program, and `127.0.0.1::9090` makes Docker choose an available macOS port. The port inside the container remains `9090`.

```sh
docker run --init --rm -it --name mini-htop -p 127.0.0.1::9090 \
  -v "$PWD":/work -w /work rust:1.94-alpine \
  cargo run --release -- --interval 1 --metrics-addr 0.0.0.0:9090
```

Leave terminal A open. In **macOS terminal B**, find the assigned local address:

```sh
docker port mini-htop 9090
```

For example, Docker may print `127.0.0.1:58513`; the port can change each time. This command asks Docker for the address and queries `/metrics`:

```sh
curl --retry 20 --retry-delay 1 --retry-connrefused -i "http://$(docker port mini-htop 9090)/metrics"
```

The first build and sample take some time, so `curl` retries while the server starts. A successful response contains `HTTP/1.1 200 OK` and `mini_htop_` metrics. `/metrics` returns Prometheus text; `--json` writes JSON lines to the program's standard output.

### 3. Stop and verify

Choose one stop method: press `Ctrl-C` in **terminal A**, or run this in **terminal B**:

```sh
docker stop mini-htop
```

Either method stops the container. Because the run command includes `--rm`, Docker removes the container after it exits. Verify from either macOS terminal:

```sh
docker ps -a --filter name=mini-htop
```

The output should contain only the column headers, with no `mini-htop` container. To **keep the stopped container**, omit `--rm` when starting it. In that case, both stop methods leave the container available, and `docker rm mini-htop` removes it later. If you started an older command without `--name mini-htop`, use `docker ps` to find its ID and run `docker stop <container-id>`.

### Optional: enter a Linux shell

To run Linux commands interactively, start a container shell from a macOS terminal:

```sh
docker run --rm -it -v "$PWD":/work -w /work rust:1.94-alpine sh
```

Only after the prompt changes to the container prompt (for example, `/work #`) should you run `uname -s`, `ls /proc/stat /sys/block`, or `cargo test`. Type `exit` to leave the container. The official Rust image does not include Rustfmt or Clippy; run `rustup component add rustfmt clippy` inside the container if you need them there.

The container process list only shows processes in its namespace. To monitor all processes and network interfaces on a Linux machine, run mini-htop directly on that Linux host or VM.

## Run on a Linux host or VM

Install Rust 1.94 or newer, copy the project to Linux, and run these commands from the project directory:

```sh
cargo test
cargo run --release
cargo run --release -- --interval 2 --alert-cpu 80 --alert-memory 90
cargo run --release -- --once --json
cargo run --release -- --once --history-file /tmp/mini-htop-samples.jsonl
tail -n 1 /tmp/mini-htop-samples.jsonl
```

Press `Ctrl-C` to stop continuous monitoring. `--json` prints one JSON object per update; `--history-file` appends timestamped JSON lines to a file; `--once` prints one sample after one interval. The HTTP server returns metrics after its first sample. Run with `--help` for all options.

## Data sources and calculations

| Metric | Linux source | Calculation |
| --- | --- | --- |
| CPU | `/proc/stat` | Busy CPU ticks divided by total CPU ticks between samples |
| Memory | `/proc/meminfo` | `MemTotal - MemAvailable` |
| Disk I/O | `/sys/block`, `/proc/diskstats` | Change in sectors for top-level block devices, excluding loop and RAM devices; 512 bytes per sector |
| Network | `/proc/net/dev` | Change in bytes for all interfaces except loopback |
| Processes | `/proc/<pid>/stat` | Change in CPU ticks and resident memory pages; shows up to 12 processes |

Disk and network rates are in bytes per second. Overall CPU usage ranges from 0 to 100%; a process can exceed 100% CPU when it uses multiple cores. Recent charts are kept in memory; use `--history-file` to save samples across runs.

Inside a container, CPU, memory, and block-device data may reflect the host, while process and network data depend on the container's namespaces. Processes may exit during sampling; unreadable processes are skipped.

## Development checks

```sh
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```
