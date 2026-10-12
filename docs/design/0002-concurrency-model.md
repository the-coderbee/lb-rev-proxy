# 0002: Concurrency Model

- **Status:** Accepted
- **Author:** the-coderbee
- **Date:** 2026-10-13

## Problem
A proxy must hold many mostly idle connections at once so that all connections can be served concurrently. The target is 100,000 concurrent client connections.

## Three proposed models

### Thread-per-connection
In this architecture a dedicated OS thread is spawned for every new connection. The thread manages all I/O operations for the entire lifespan of the connection.

### Thread pool
In this architecture a fixed number of threads is spawned up front. Connections wait in a queue and are assigned to threads as threads become free.

### Async I/O with event loop
In this architecture the responsibility of multitasking is shifted from the OS to the application layer. The application runs a continuous loop that asks the OS for I/O events using interfaces like `epoll` (Linux). When one connection is waiting for data, the thread works on another, so one thread can serve many connections. Multi-threaded runtimes run one such loop per worker thread, typically one worker per CPU core.


## Comparison: thread-per-connection vs async I/O with event loop (using Tokio)
### Test setup

- Program A (`pa`): spawns N OS threads with `std::thread::Builder`,
  each sleeping 30 s.
- Program B (`pb`): spawns N Tokio tasks with `JoinSet`, each awaiting
  `tokio::time::sleep` for 30 s (multi-threaded runtime, default settings).
- Built with `cargo build --release`; measured with `/usr/bin/time -v`.
- Machine: <CPU, RAM, Linux kernel version, rustc version, tokio version>

### Raw measurements

| Program | N | Result | Max RSS (KB) | Minor page faults | Voluntary ctx switches | Involuntary ctx switches | User time (s) | System time (s) |
|---|---:|---|---:|---:|---:|---:|---:|---:|
| pa (threads) | 1 | ok | 2,296 | 97 | 3 | 1 | 0.00 | 0.00 |
| pa (threads) | 10,000 | ok | 98,060 | 24,293 | 19,762 | 21 | 0.09 | 0.47 |
| pa (threads) | 100,000 | **failed** after 18,639 threads (`EAGAIN`, os error 11) | — | — | — | — | — | — |
| pb (tasks) | 1 | ok | 3,396 | 182 | 76 | 2 | 0.00 | 0.00 |
| pb (tasks) | 10,000 | ok | 6,796 | 1,218 | 2,815 | 111 | 0.02 | 0.03 |
| pb (tasks) | 100,000 | ok | 43,332 | 10,422 | 29,459 | 974 | 0.25 | 0.31 |

### Derived figures

Marginal cost per unit = (RSS at N − RSS at 1) ÷ (N − 1).

| Program | N | Memory per unit | Voluntary ctx switches per unit |
|---|---:|---:|---:|
| pa (threads) | 10,000 | 9.58 KB | 1.98 |
| pb (tasks) | 10,000 | 0.34 KB | 0.28 |
| pb (tasks) | 100,000 | 0.40 KB | 0.29 |

| Comparison | Value |
|---|---:|
| Memory per thread ÷ memory per task (N = 10,000) | ≈ 28× |
| Extra fixed cost of `pb` over `pa` (RSS at N = 1) | 1,100 KB |
| Break-even point: 1,100 KB ÷ (9.58 KB − 0.34 KB) | ≈ 119 connections |

### Thread limit

| Measurement | Value |
|---|---:|
| Threads created by `pa` before failure | 18,639 |
| Session cgroup task limit (`pids.max`) | 18,726 |
| Difference (other tasks already in the session) | 87 |

## Decisions

### D1: Concurrency model
- **Decision:** Use async (non-blocking) I/O with an event loop.
- **Alternatives:**
  - Thread-per-connection.
  - Thread pool.
- **Reason:**
  - Thread-per-connection does not reach the target. It stopped at 18,639 threads with `EAGAIN`, because of the session's cgroup task limit. An operator can raise that limit, but every limit raised still costs about 9.58 KB of memory and about 2 context switches per thread. Tokio tasks reached 100,000 with no tuning, at about 0.34–0.40 KB and about 0.3 context switches per task.
  - Each thread needs its own stack, but most connections spend the vast majority of their time idle while still holding it.
  - A thread pool is rejected by reasoning: idle connections hold threads, so with a 100-thread pool, connection 101 waits while the CPU sits idle.
- **Consequences:**
  - Higher fixed cost. At N = 1, the async program used 1,100 KB more memory than the threaded one. It pays for itself after about 119 connections.
  - A task gives control back to the runtime only at an `.await`. If a task runs a long computation or a blocking call without reaching one, other tasks queued on the same worker thread are delayed until it yields. All proxy code must avoid blocking calls inside async code.
  - Async code is harder to write and debug than threaded code. Stack traces are less readable, and a stuck task produces no error.

### D2: Runtime
- **Decision:** Use Tokio as the async runtime.
- **Alternatives:**
  - Build my own event loop on `mio`, a thin cross-platform wrapper over `epoll` (Linux), `kqueue` (macOS), and IOCP (Windows). Rejected: by far the most work, and the runtime is not the product.
  - `smol`, a small, modular runtime built from components such as `async-io`, `async-executor`, and `async-task`. Rejected: smaller ecosystem, and HTTP libraries support it less directly.
  - `async-std`. Rejected: officially discontinued in favor of `smol`.
  - Thread-per-core runtimes such as `glommio` and `monoio`, which run an independent event loop on each core and use `io_uring`. Rejected for v1: Linux-only, smaller ecosystems, and they need a different, share-nothing design.
- **Reason:**
  - Tokio is a multi-threaded runtime with work stealing. When one worker thread runs out of tasks, it takes tasks from another's queue, which reduces idle cores when some connections are busier than others.
  - It provides everything v1 needs: TCP listener and streams, timers (connect timeout, health checks), and synchronization primitives.
  - Since version 1.0, `hyper` (the HTTP library planned for the Layer 7 phase) is not tied to a specific runtime, but Tokio is its best-supported runtime through the `hyper-util` crate, and most of the surrounding ecosystem is built on it.
  - It is the most widely used and actively maintained Rust async runtime.
- **Consequences:**
  - Lock-in. The proxy will use Tokio's own types (`TcpStream`, `sleep`, and so on), which only work with Tokio's I/O driver and timer. Switching runtimes later means rewriting all I/O and timing code.
  - Because work stealing moves tasks between threads, futures passed to `tokio::spawn` must be `Send + 'static`, and state shared between tasks must be thread-safe (`Arc`, `Mutex`, atomics).
  - Tokio is a large dependency: longer compile times and a larger binary. Enabling only the features the proxy uses reduces this.

## Limitations
- Threads and tasks only slept. No data was read, written, or buffered. Real connections hold buffers that cost the same in both models, so the real per-connection gap is smaller than 28×.
- Context-switch cost was measured with idle threads, not under real load.
- One machine, one run per configuration.
- The thread pool was evaluated by reasoning, not measured.
- Each proxied connection needs two sockets, one to the client and one to the backend, and every socket is a file descriptor. The experiment did not open a single socket, so it could not reveal file descriptor limits.

## Open Questions

- **File descriptor limit.** Each proxied connection uses 2 file descriptors
  (client and backend). On the test machine the soft limit (`ulimit -n`) is
  1,024, which allows about 510 concurrent connections, far below the
  100,000 target. The hard limit (`ulimit -Hn`) is <your value>.
  Should the proxy raise its own soft limit at startup, or leave this to
  the operator (`ulimit -n`, systemd `LimitNOFILE=`)?
- **Running out of file descriptors.** When the limit is reached, `accept()`
  fails with `EMFILE`. Retrying immediately would spin a CPU core while the
  proxy is already overloaded. How should the proxy behave in this case?
- Verify the 100,000-connection target with real sockets and a load test in
  a later phase.
