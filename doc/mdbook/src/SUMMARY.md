# Summary

[Introduction](introduction.md)

# User Guide

- [Chapter 1: Basics of DES Simulation](chapter_01/README.md)
  - [Quick Start](chapter_01/quick_start.md)
  - [Basics of Discrete Event Simulation](chapter_01/des_basics.md)
  - [The Metrics Collection System](chapter_01/metrics_overview.md)

- [Chapter 2: DES-Core: The Core Abstractions](chapter_02/README.md)

- [Chapter 3: DES-Components: Components for Server Modeling](chapter_03/README.md)

- [Chapter 4: Metrics Collection](chapter_04/README.md)

- [Chapter 5: Example: M/M/k Queues](chapter_05/README.md)
  - [Basic M/M/1 and M/M/k Queues](chapter_05/basic_queues.md)
  - [Clients with timeouts and retry policies](chapter_05/retry_policies.md)
  - [Clients with admission control](chapter_05/admission_control.md)

- [Chapter 6: Tower and Rust Server Infrastructure](chapter_06/README.md)
  - [Common abstractions](chapter_06/abstractions.md)
  - [Small examples of tower servers](chapter_06/examples.md)
  - [Multiple servers (App -> DB, LB -> App -> DB)](chapter_06/multi_tier.md)

- [Chapter 7: Advanced examples](chapter_07/README.md)
  - [Service time distributions that depend on payload](chapter_07/payload_service_times.md)
  - [Client-side admission control](chapter_07/client_admission_control.md)
  - [Server-side throttling and admission control](chapter_07/server_throttling.md)
  - [Advanced workload generation patterns](chapter_07/workload_generation.md)

# Reference

- [API Documentation](reference/api.md)
- [Configuration Reference](reference/configuration.md)
- [Migration Guides](reference/migration.md)