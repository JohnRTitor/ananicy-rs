# CPU Topology and Affinity

`ananicy-rs` allows you to pin workloads to specific logical CPUs using the `cpuset` attribute in your rules. It natively understands complex CPU topologies, including heterogeneous cores (big.LITTLE) and AMD X3D V-Cache processors.

## Direct Core Pinning

You can pin to specific cores directly using standard Linux cpuset notation:
```json
{"name": "my-app", "cpuset": "0-3,8-11"}
```

## Named Aliases

Instead of hardcoding CPU IDs, `ananicy-rs` provides named aliases that are resolved dynamically at runtime based on the system's auto-detected topology.

| Alias | Description |
|-------|-------------|
| `big-cores` | High-performance cores (Big + BigTurbo) |
| `little-cores` | Efficiency cores (Little) |
| `turbo-cores` | Highest-capacity cores only (BigTurbo) |
| `performance-cores` | `turbo-cores` if available, otherwise `big-cores` |
| `efficiency-cores` | Same as `little-cores` |
| `all-cores` | All online CPUs |
| `x3d-cache` | CPUs on the LLC(s) with the largest L3 cache (works on any CPU) |
| `x3d-frequency` | AMD X3D high-frequency CCD (X3D only) |
| `llc-N` | CPUs sharing LLC domain N (e.g., `llc-0`, `llc-1`) |
| `node-N` | CPUs on NUMA node N (e.g., `node-0`, `node-1`) |

*On systems without heterogeneous cores, `big-cores` and `all-cores` resolve to the same set of CPUs.*

### Heterogeneous Topology Detection

At startup, `ananicy-rs` probes `sysfs` to build a full CPU topology:
- **NUMA nodes** and **LLC (last-level cache) domains** are identified and grouped.
- **Core types** (BigTurbo, Big, Little) are classified using CPU capacity values from `sysfs`. Five capacity sources are probed in priority order: `amd_pstate_prefcore_ranking`, `amd_pstate_highest_perf`, `acpi_cppc/highest_perf`, `cpu_capacity`, `cpuinfo_max_freq`.
- **big.LITTLE detection**: If the highest-capacity CPU exceeds 1.3x the lowest, the system is classified as heterogeneous and cores are split into Big/Little groups.
- **SMT** (simultaneous multithreading) status is detected.

Example usage to restrict a background task to efficiency cores:
```json
{"name": "file-indexer", "cpuset": "efficiency-cores", "nice": 19, "sched": "idle"}
```

To pin a workload to performance cores (Intel P-cores or AMD high-capacity cores):
```json
{"name": "my-game", "cpuset": "performance-cores", "nice": -5}
```

## AMD X3D Support

On AMD X3D CPUs (e.g., 7800X3D, 7950X3D, 9800X3D, 9950X3D), `ananicy-rs` automatically detects the V-Cache and frequency CCDs at startup by reading CPU topology and L3 cache sizes from `sysfs`.

To pin a game to the V-Cache CCD:
```json
{"name": "game-x", "cpuset": "x3d-cache", "nice": -5}
```

The `x3d_mode` configuration option in `ananicy.conf` controls the `amd_x3d_vcache` kernel driver (if present):
- `auto`: Do not change the driver mode (default).
- `cache`: Set the driver to prefer the V-Cache CCD for scheduling.
- `frequency`: Set the driver to prefer the high-frequency CCD for scheduling.
