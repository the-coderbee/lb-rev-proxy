# 0001: v1 Problem Statement

- **Status:** Draft
- **Author:** the-coderbee
- **Date:** 2026-10-08

## Problem

Applications that run on several servers need a single entry point that spreads
traffic across those servers and stops sending traffic to servers that have
failed. Without one:

- Clients must know every server's address and choose one themselves.
- Clients keep connecting to a dead server, because nothing tells them it is down.
- Load spreads unevenly, so some servers are overloaded while others sit idle.

This proxy sits in front of the servers and solves these problems for TCP traffic.

## Users

- **Client:** sends traffic through the proxy. Needs its connection to reach a
  working backend with minimal added latency. Does not need to know that
  backends exist.
- **Operator:** runs the proxy and lists the backends in a config file. Needs to
  know when a backend goes down or comes back.

## Goals (v1)

- Accept TCP connections on a single listening address and forward each one to a
  backend (Layer 4).
- Distribute new connections across healthy backends using round robin.
- Detect failed backends using active health checks (periodic TCP connect) and
  passive health checks (failed real connection attempts), and skip backends
  marked down.
- Retry a failed connection attempt on a different backend.
- Enforce a connect timeout on every backend connection attempt.
- Read the listening address and backend list from a config file.
- Log backend state changes (down / up) so the operator can see them.

## Non-goals

- **Layer 7 (HTTP) handling.** Routing by path or header requires parsing HTTP,
  which is a separate body of work. Planned for a later phase.
- **Weighted or load-aware balancing.** v1 assumes backends have similar
  capacity. Weighted least connections replaces round robin once that
  assumption no longer holds.
- **TLS termination.** v1 forwards bytes as-is and does not handle encryption.
- **Config hot reload.** Changing the backend list requires a restart in v1.
- **Metrics.** v1 reports through logs only. Prometheus metrics come later.
- **DDoS protection and rate limiting.** A separate and much harder problem.
- **Moving or replaying in-flight connections.** See D4.

## Success Criteria

- **Latency:** p99 latency overhead added by the proxy is under 1 ms. Overhead is
  measured as the difference between connecting directly to a backend and
  connecting through the proxy, on the same network.
- **Distribution:** with N healthy backends and N × k connections, each backend
  receives exactly k connections. For example, 3 backends and 3,000 connections
  gives 1,000 each.
- **Failure handling:** once a backend is marked down, no new client connection
  is sent to it. Connection attempts that reach a down backend before it is
  marked down are retried on another backend (D5).

## Failure Behavior

| Situation | Proxy behavior | What the client sees |
|---|---|---|
| Backend dies while a connection is open | Close the client connection (D4) | Connection reset; must reconnect |
| Connect attempt fails (refused or timed out) | Count it as a health failure and try the next healthy backend (D5) | Nothing, apart from up to 300 ms of extra delay per failed attempt |
| Backend fails between active health checks | Handled by the row above: real connection attempts detect it | Same as above |
| Backend recovers | Mark it up after active health checks succeed (threshold: open question) | Nothing |
| All backends down | Accept, then immediately close the client connection; log an error for the operator | Connection closed. An L4 proxy cannot send an HTTP error page. |

## Decisions

### D1: Operate at Layer 4 (TCP) in v1

- **Decision:** v1 forwards raw TCP connections without parsing HTTP.
- **Alternatives:** Layer 7, parsing HTTP so requests can be routed individually
  based on their contents.
- **Reason:** L4 is simpler and still solves the core problem of spreading
  connections across servers. L7 adds HTTP parsing, which is a separate body of
  work.
- **Consequences:** The proxy cannot route by URL or header, cannot balance
  individual requests within one connection, and cannot detect backends that
  accept connections but return errors.

### D2: Round robin

- **Decision:** Distribute new connections across healthy backends in a fixed
  rotating order.
- **Alternatives:** Weighted least connections, which sends each connection to
  the backend with the lowest ratio of open connections to capacity weight.
- **Reason:** Round robin needs only a single counter. Weighted least
  connections needs live per-backend connection counts and a weight for each
  backend. Since v1 assumes backends have similar capacity, the extra
  complexity does not buy anything yet.
- **Consequences:** Load becomes uneven when backends differ in capacity, or when
  connections differ in duration (a long download and a quick request count the
  same).

### D3: Active and passive TCP health checks

- **Decision:** A background task periodically opens a TCP connection to each
  backend (active check). Failed real connection attempts also count as health
  failures (passive check). A backend that fails is marked down and skipped
  until active checks succeed again.
- **Alternatives:**
  - Check a backend before every client connection. Rejected: it doubles the
    connections opened to each backend and adds latency to every client.
  - HTTP health checks that verify a valid response. Rejected for v1: requires
    Layer 7 (D1).
- **Reason:** A TCP connect is the cheapest check that detects a backend that has
  stopped or is unreachable, and it matches the Layer 4 scope. Passive checks
  catch failures between active checks at no extra cost.
- **Consequences:** Cannot detect a backend that accepts connections but is
  broken (hung application, wrong service on the port). Detection by active
  checks is delayed by up to one check interval.

### D4: Abort open connections when their backend dies

- **Decision:** When a backend dies while a connection is open, the proxy closes
  the client's connection.
- **Alternatives:** Buffer the data sent on each connection and replay it to
  another backend. Rejected: the new backend has none of the original
  backend's state, and replaying can repeat an operation that must happen only
  once, such as a payment.
- **Reason:** The proxy cannot know whether the dead backend already acted on the
  data, so only the client can decide whether retrying is safe.
- **Consequences:** The client sees a connection reset and must reconnect and
  retry on its own. Long-lived connections (large downloads, WebSockets) are
  lost when their backend dies.

### D5: Retry on connect failure

- **Decision:** When a connection attempt to a backend fails (refused or timed
  out), try the next healthy backend. Each backend is tried at most once per
  client connection. If every backend fails, close the client connection and
  log an error. Each failure counts toward marking that backend down.
- **Alternatives:**
  - Retry the same backend. Rejected: a backend that just refused or timed out
    will most likely fail again.
  - Do not retry. Rejected: the client sees an error even though healthy
    backends exist.
- **Reason:** No client data has reached any backend before the connection is
  established, so trying another backend is always safe.
- **Consequences:** In the worst case, a client waits one connect timeout per
  failing backend. With 3 backends where 2 time out, that is up to 600 ms before
  reaching the third.

### D6: 300 ms connect timeout

- **Decision:** A connection attempt to a backend is abandoned if it has not
  succeeded within 300 ms. The value is configurable, with 300 ms as the default.
- **Alternatives:**
  - 2–3 seconds. Rejected: a backend that silently drops packets would stall
    every client routed to it for 2–3 seconds, thousands of times the 1 ms
    latency target.
  - Very short, such as 10 ms. Rejected: a backend that is briefly slow under
    load would be treated as failed.
- **Reason:** Connecting within the same datacenter takes about 0.1–0.5 ms, so
  300 ms leaves hundreds of times that as headroom while still failing fast.
- **Consequences:** A backend that silently drops packets adds up to 300 ms of
  delay to each connection routed to it before it is marked down. The value has
  not been validated under real load.

### D7: Backend list from a config file

- **Decision:** The listening address, backend list, and settings such as the
  connect timeout are read from a config file at startup.
- **Alternatives:**
  - Command-line arguments. Rejected: workable for two or three backends, but
    unwieldy as the list and the number of settings grow.
  - Service discovery (DNS, Consul, Kubernetes), where the proxy asks a
    registry which backends exist. Rejected for v1: adds an external
    dependency and much more complexity.
- **Reason:** A file holds structured settings, can be version-controlled, and
  scales to many backends.
- **Consequences:** Adding or removing a backend requires editing the file and
  restarting the proxy, which drops all open connections.

## Open Questions

- How often do active health checks run?
- How many consecutive failures mark a backend down?
- How many consecutive successes mark it up again?
- Do passive failures count toward the same threshold as active ones?
- Which config file format? (TOML is the convention in Rust.)
- How will the latency overhead be measured (benchmark setup and tooling)?
