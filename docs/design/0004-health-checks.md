# 0004: Health Checks

- **Status:** Accepted
- **Author:** the-coderbee
- **Date:** 2026-10-26

## Problem
The proxy currently learns that a backend is down only when a client connection to it fails. A dead backend therefore keeps its turn in the rotation: every client whose turn it is waits for a failed connection attempt before being retried on the next backend, and that next backend receives double load (0003, D5). Nothing marks the backend as down, so this continues until the backend recovers, and nothing tells the operator.

## Goals
- A backend that stops accepting connections is no longer chosen for new connections within about 6 seconds, with default settings.
- A recovered backend starts receiving connections again within about 4 seconds with default settings, without operator action.
- A single failed check or dropped connection does not mark a healthy backend down. A backend that fails intermittently doesn't change modes on every check.
- If every backend is marked down, the proxy still tries every backend, rather than refusing clients.
- Choosing backends for a connection never waits on a lock: reading a backend's health is a single atomic load, and health updates never block connections.
- Health checks are spread evenly over time, rather than firing for every backend at the same instant.
- The operator is told once when a backend goes down and once when it comes back.
- Settings are configurable, and invalid values (a threshold of 0, a check timeout longer than the interval) are rejected at startup.

## Non-goals
- **HTTP health checks:** detecting backends that accept connections but are broken requires requesting something like `/health` and checking the response, which needs Layer 7 (Phase 6).
- **Closing existing connections to a backend marked down:** marking a backend down only affects new connections. Open connections may still be working, and the health checks may be wrong, so cutting them could break healthy sessions (see 0001, D4).
- **Detecting slow backends:** TCP checks only verify that a backend accepts connections. Detecting slowness needs latency measurements on real traffic, which belongs with a latency-aware balancing algorithm.
- **Adaptive check intervals:** checking down backends more often and up ones less often. At the default interval the cost of checks is negligible (see D6), so there is no need yet.
- **Many proxies checking the same backends:** v1 is designed for a single proxy. With many proxies, the check load on each backend grows with the number of proxies, which would need a shared health service or longer intervals.
- **Exposing health state** other than through logs, such as metrics or a status page. Planned for the observability phase.
- **Changing the backend list at runtime:** planned for Phase 4. Each backend now has a checker task, which would need to start and stop with it.

## Decisions

### D1: Health state as one signed atomic per backend
- **Decision:** Each backend's health is a single signed integer in an atomic. The sign is the mode: zero or positive means Up, negative means Down. The value is the current streak: failures in a row while Up, successes in a row while Down. Updates go through `fetch_update` with a pure `next_state(current, event, thresholds)` function.
- **Alternatives:**
  - Separate atomics for the mode and the counts. Rejected: they are updated independently, so concurrent updates can leave a mode that disagrees with its count.
  - A mutex per backend. Rejected: every connection reads health, so every connection would lock it.
- **Reason:** One value means the mode and the streak can never disagree. `fetch_update` computes each new value from the true current value and retries if another task changed it in the meantime, so no update is lost and no task waits for a lock. Keeping the rules in a pure function means every threshold rule can be unit tested without threads or a network.
- **Consequences:**
  - The encoding is not obvious to a new reader, so `next_state` needs clear documentation and thorough tests.
  - Under contention, an update may be recomputed a few times. Each attempt is a few arithmetic operations, so this is negligible.

### D2: Thresholds of 3 failures down and 2 successes up, in a row, configurable
- **Decision:** A backend is marked down after 3 consecutive failures, and up again after 2 consecutive successes. Both thresholds are fields in `ProxyConfig`.
- **Alternatives:**
  - Switch on a single failure. Rejected: one dropped packet would mark a healthy backend down (false positives).
  - The same threshold in both directions. Rejected: a struggling backend could switch modes on every check (flapping).
  - Hardcoded constants. Rejected: operators need different trade-offs between fast failover and tolerance of flaky networks.
- **Reason:** The fall threshold trades false positives against detection time, which is roughly the threshold times the check interval. Different thresholds in each direction prevent flapping. 3 and 2 are common defaults (HAProxy uses the same) and give about 6 seconds to detect a failure and about 4 seconds to recover with the default 2 second interval.
- **Consequences:**
  - A threshold of 0 has no meaning, so the config now has values that must be rejected (D10).
  - A dead backend may still be chosen for a few seconds before it is marked down. Clients sent to it are retried on the next backend.

### D3: Passive checks
- **Decision:** Real client connections report to the backend's health: a failed connection attempt counts as a failure, and a successful one as a success.
- **Alternatives:** Active checks only. Rejected: a dead backend would keep being tried until the checker noticed, even when real traffic had already shown the failure.
- **Reason:** Real traffic detects failures at the moment they happen, between active checks, at no extra cost. A successful connection proves the backend is working, so it resets the failure streak.
- **Consequences:**
  - A backend marked down receives no real traffic, so only active checks can bring it back up.
  - A passive failure counts the same as a failed active check. Under heavy traffic, a dead backend can be marked down within a few client connections, well before the checker's next round.

### D4: Fail open when every backend is marked down
- **Decision:** If every backend is marked down, `candidates()` ignores health and returns all of them, and the retry loop tries each one as before.
- **Alternatives:** Fail closed: return no candidates and close the client immediately.
- **Reason:** When every backend is marked down, either all of them really are down, or the health checks are wrong (the checker's network is broken, every backend restarted at the same moment, or a firewall blocks the probes but not real traffic). If they really are down, both options give clients the same result. If the checks are wrong, failing closed turns a monitoring problem into a complete outage, while failing open still serves clients. Telling the operator is the job of the logs, not of refusing clients.
- **Consequences:** If every backend really is down, each client waits through a connection attempt to every backend before being closed. Refused connections fail instantly, but unreachable backends cost up to one connect timeout each.

### D5: Backends start Up
- **Decision:** Every backend starts in the Up mode with no failures.
- **Alternatives:**
  - Start Down until checks pass.
  - Start Down and run a round of checks before accepting connections.
- **Reason:** Proxies restart on every deployment and configuration change, so any startup window without service would repeat regularly. Starting Down together with fail open (D4) would pass through an "all backends down" state on every restart, logging an error each time, and errors that appear on every normal restart train operators to ignore them. Starting Up gives the same behavior as before health checks existed, and passive checks and retries cover any backend that is actually dead.
- **Consequences:** A backend that is dead at startup receives a few connections before it is detected. Those clients are retried on the next backend.

### D6: One checker task per backend
- **Decision:** Each backend has its own background task that waits for the interval, checks the backend with a TCP connection attempt, and reports the result. The interval defaults to 2 seconds and the check timeout to 1 second, both configurable.
- **Alternatives:**
  - One task that checks all backends one after another. Rejected: a backend that hangs until the check timeout delays the checks of every backend after it.
  - One task that checks all backends concurrently in rounds. Workable, but needs coordination that independent tasks avoid.
- **Reason:** A slow backend only delays its own checks, and each task is a simple loop. The check timeout has its own setting because checks run in the background and can tolerate a different value than client connections. The interval is derived from the detection target in the goals.
- **Consequences:**
  - The number of tasks grows with the number of backends. Tasks are cheap, so this is acceptable.
  - Each check opens a TCP connection to the backend and briefly uses a file descriptor. At the default interval this is one connection every 2 seconds per backend, negligible next to real traffic.

### D7: Random jitter before each checker's first check
- **Decision:** Each checker task waits a random delay between zero and one interval before its first check.
- **Alternatives:**
  - No jitter. Rejected: every checker starts at the same moment with the same interval, so all checks fire together in bursts.
  - Jitter before every check. Possible, but the first delay already spreads the checks out, and tasks with equal intervals stay spread.
- **Reason:** Spreading checks evenly avoids synchronized bursts on the proxy and on the backends, which grow worse when many backends or many proxies restart together.
- **Consequences:**
  - Check timing is unpredictable, so tests must be able to disable jitter through the config.
  - The first active check may happen up to one interval after startup. With backends starting Up (D5), this does not affect clients.

### D8: Backends stored as `Vec<Arc<Backend>>`, and `candidates()` yields `&Backend`
- **Decision:** A `Backend` struct holds an address and its health. The balancer stores each one in an `Arc`. `candidates()` yields references to the backends themselves, and each checker task owns a clone of its backend's `Arc`.
- **Alternatives:**
  - Yield addresses, and search the list for the matching backend on every report. Rejected: an extra search for information the caller already had.
  - Give each checker an `Arc<RoundRobin>` and an index. Rejected: each checker would depend on the whole balancer to reach one backend, and indexes become fragile once the backend list can change at runtime.
- **Reason:** Callers report results directly to the right backend. Each checker owns exactly what it needs, which also satisfies the `'static` requirement of `tokio::spawn`.
- **Consequences:**
  - One extra pointer hop to reach each backend, which is negligible.
  - `Backend` becomes a type that callers of the balancer use, so its methods need clear documentation.

### D9: Log only when a backend changes mode
- **Decision:** Log once at `warn` when a backend goes from Up to Down, and once at `info` when it goes from Down to Up, including the backend address. Individual check results are logged at `debug`.
- **Alternatives:** Log every check result at the default level. Rejected: at a 2 second interval, that is 1,800 lines per hour per backend, which buries the events that matter.
- **Reason:** A mode change is what the operator needs to act on. `fetch_update` returns the previous value, so comparing the old and new modes shows exactly when a transition happened, and only the task that caused it logs it.
- **Consequences:** At the default log level, individual failed checks are invisible. An operator investigating a problem can enable them with `RUST_LOG=debug`.

### D10: Config validation at startup
- **Decision:** The configuration is validated when the proxy starts: both thresholds must be at least 1, and the check timeout must be shorter than the check interval.
- **Alternatives:** Accept any values. Rejected: a threshold of 0 or overlapping checks would only show up later, as confusing behavior at runtime.
- **Reason:** Invalid values are caught once, at the boundary where configuration enters the program, with a clear error, the same principle as rejecting an empty backend list (0003, D4).
- **Consequences:** `ProxyConfig` has public fields, so an invalid config can exist until it is validated. Validation must happen before the proxy starts, and tests that build configs must pass it too.

## Testing
- Unit tests for `next_state`: traces through every rule, including streaks being reset and both mode switches.
- Unit tests for config validation: thresholds of 0 and a check timeout longer than the interval are rejected; the defaults are accepted.
- A unit test that `candidates()` skips backends marked down.
- A unit test that `candidates()` returns every backend when all are marked down (fail open).
- An integration test where a backend stops: with a short interval and no jitter, it stops being chosen.
- An integration test where a stopped backend starts again: it is chosen again.

## Open questions
- Is a 1 second default check timeout right, or should it be closer to the connect timeout?
- Should a check that is still running when the next interval arrives be skipped, or is the timeout-shorter-than-interval rule enough?
