# 0003: Load Balancing

- **Status:** Accepted
- **Author:** the-coderbee
- **Date:** 2026-10-18

## Problem
When there are multiple backends, we need a way to distribute all incoming connections across the backends. When many tasks are running concurrently on different threads there needs to be a way to safely access the backends list without causing data races.

## Goals
- Allow multiple backends to be used with the proxy.
- Round robin for distributing connections across all backends.
- Retry on connect failure, trying each backend at most once per client connection (0001, D5).
- Make the connect timeout configurable through a settings struct, defaulting to 300 ms (0001, D6).

## Non-goals
- **Health checks:** planned for Phase 3. Until then, a dead backend is only discovered when a connection to it fails.
- **Weighted balancing:** requires assigning weights to the backends so connections can be routed based on each backend's capacity. Planned as the upgrade from round robin (0001, D2).
- **Changeable backend list:** handling backends added or removed after startup needs separate work (Phase 4, config reload).

## Decisions

### D1: Shared counter as an atomic
- **Decision:** Keep the round-robin position in an `AtomicUsize` that only ever increases. Each connection calls `fetch_add(1)` and picks `backends[counter % backends.len()]`.
- **Alternatives:**
  - `Mutex<usize>`.
  - Resetting the counter to 0 when it reaches the number of backends, instead of using modulo.
- **Reason:**
  - Every connection passes through this counter, so it sits on the proxy's hot path. `fetch_add` performs the read-and-increase as one indivisible operation without locks, so tasks never wait on each other. A mutex would make a burst of new connections queue up behind the lock.
  - Resetting requires a check followed by a conditional write, which reintroduces the data race (or needs a mutex or a compare-and-swap loop). With modulo, the only shared operation is `fetch_add`, and the wrapping happens in each task's local arithmetic.
  - Atomics allow updating through a shared reference (`&self`), which is required because the counter is shared through an `Arc` (D2).
- **Consequences:**
  - An atomic protects exactly one value. If a future algorithm needs several values updated together (for example, weighted least connections updating per-backend counts while choosing a backend), it will need a mutex or a different design.
  - The counter wraps to 0 after `usize::MAX` connections, making one rotation slightly uneven at that moment. On 64-bit systems this is negligible in practice.

### D2: Sharing state between tasks with `Arc`
- **Decision:** Share the balancer between connection tasks through an `Arc`.
- **Alternatives:**
  - Plain references (`&backends`).
  - `Rc`.
  - Copying the backend list into each task.
- **Reason:**
  - Plain references are rejected by `tokio::spawn`, which requires everything a task holds to be `'static`. A spawned task can outlive the function that owns the list.
  - `Rc` updates its reference count with ordinary arithmetic, so it is not `Send` and cannot cross threads. `Arc` updates its count atomically, which is safe across threads.
  - Copying the list costs memory and time on every connection. Cloning an `Arc` only copies a pointer and increments a count.
- **Consequences:**
  - Cloning and dropping handles is slightly slower than with `Rc`.
  - Data behind an `Arc` is read-only. Anything that must change at runtime needs interior mutability (atomics or a mutex).

### D3: A `RoundRobin` type that hides the list and counter
- **Decision:** A `RoundRobin` struct owns the backend list and the counter as private fields, and exposes methods that answer which backends to try and in what order.
- **Alternatives:** Exposing the fields to callers.
- **Reason:** Callers never deal with the counter, the modulo or the memory ordering. All the logic stays in one place and is impossible to get wrong elsewhere. The rotation can be unit tested without any network. When a different algorithm replaces round robin, only the inside changes.
- **Consequences:**
  - Callers cannot inspect or adjust the internals.
  - When Phase 3 adds health status, the type must grow to hold it, or be replaced by a trait with several implementations.

### D4: Empty backend list is rejected at construction
- **Decision:** `RoundRobin::new` returns an error for an empty backend list. It is the only way to create a `RoundRobin`.
- **Alternatives:**
  - Return an `Option` from every pick, so every caller handles "no backend" on every connection.
  - Check for an empty list on every connection.
- **Reason:** `main` creates the `RoundRobin` at startup, so an empty list fails immediately with a clear message. This makes the invalid state unrepresentable: the check happens once, at the boundary where data enters the program, and everything after it can rely on at least one backend existing. It also removes the division by zero in `counter % 0`.
- **Consequences:**
  - The proxy can never start with zero backends, even deliberately.
  - When Phase 4 adds reloading the backend list, a reload to an empty list must also be rejected, or the guarantee breaks.

### D5: Retry loop lives in `handle_client` and the counter advances once per connection
- **Decision:** `RoundRobin` provides the candidates for a connection in order. `handle_client` tries to connect to each in turn. The shared counter advances once per connection to get a starting position, and the list is walked locally from there.
- **Alternatives:**
  - Retry inside the balancer.
  - Call `next_backend` again on every retry, advancing the shared counter once per attempt.
- **Reason:**
  - Retrying means connecting: async I/O, the connect timeout, `ConnectError` and logging. Keeping that in `handle_client` keeps `RoundRobin` pure logic with no network, so it stays easy to test and to replace.
  - Advancing the shared counter on every retry was rejected because other tasks advance it at the same time, so a retry could land on the same backend again or skip one. Walking the list locally from one starting position guarantees each backend is tried at most once.
- **Consequences:**
  - Load becomes uneven when a backend is down. With backends A, B and C and B down, every client whose turn is B retries on C, so C receives its own share plus B's. Health checks (Phase 3) fix this by skipping B before it becomes anyone's turn.
  - A client may wait up to one connect timeout per unreachable backend before reaching a working one.

### D6: Reporting when all backends fail
- **Decision:** Log each failed attempt as it happens, with its full cause. When every backend fails, return a summary error that records how many backends were tried, and close the client connection.
- **Alternatives:** Collect every `ConnectError` into the final error, so one log line contains all failures.
- **Reason:**
  - Each attempt's cause is already visible in the log when it happens, so repeating all of them in the final error duplicates information.
  - `std::error::Error::source()` holds a single cause, so a list of errors does not fit the error chain and would need custom formatting.
  - The final log line stays the same size however many backends are configured.
- **Consequences:**
  - Reconstructing what happened to one client requires reading several log lines. The connection span (`client=...`) ties them together.
  - Under load with every backend down, each client produces one log line per backend plus the summary.

### D7: Settings grouped in a config struct
- **Decision:** Settings are grouped in a config struct passed to `run`, starting with the connect timeout. The struct implements `Default`, with the connect timeout defaulting to 300 ms.
- **Alternatives:** Passing each setting to `run` as a separate parameter.
- **Reason:** New settings can be added without changing the signature of `run` or updating every caller, including tests. With `Default`, a test sets only the values it cares about, for example a short connect timeout. The config holds plain settings chosen once at startup. Runtime state that changes on every connection, like the round-robin counter, stays in `RoundRobin`.
- **Consequences:**
  - Every connection task needs access to the settings. While the struct holds only small `Copy` values like `Duration`, it can be copied into each task. Once it holds larger values, it should be shared through an `Arc`.
  - Default values now live in code and must stay consistent with the design docs.

## Testing
- A unit test for the rotation order with no network: three backends and six picks give A, B, C, A, B, C.
- A unit test that `RoundRobin::new` rejects an empty list.
- Integration tests with several echo servers, checking that consecutive connections reach different backends.
- A test where one backend is down and the retry reaches the next one.
- A test where every backend is down and the client connection is closed.
- A short connect timeout set through the config.

## Open questions
- At what level should each individual failed attempt be logged, given that the summary error is also logged?
- Is the uneven load from D5 acceptable until health checks exist in Phase 3?
- Where should default setting values be documented as the config struct grows?
