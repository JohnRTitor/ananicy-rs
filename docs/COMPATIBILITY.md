# Migrating from ananicy-cpp

`ananicy-rs` reads the same rule, type and cgroup files and accepts the same
configuration keys, plus a few new ones. You can usually copy `/etc/ananicy.d`
unchanged. This page lists the differences that can matter when you switch.
The full behavioural audit — what was verified equivalent, what this daemon
adds, and what it deliberately does not reproduce — is in
[internals/COMPATIBILITY.md](./internals/COMPATIBILITY.md).

## Names and paths

- The binary is `ananicy-rs` and the unit is `ananicy-rs.service`, so both
  daemons can run side by side.
- Config stays at `/etc/ananicy.d/ananicy.conf` and `/etc/ananicy.d`, with the
  same `*.rules`, `*.types` and `*.cgroups` files.
- **Set the environment variables again.** `ANANICY_CPP_CONF` and
  `ANANICY_CPP_CONFDIR` are not read. Use `ANANICY_RS_CONF` and
  `ANANICY_RS_CONFDIR`, or `--config` and `--config-dir`. An old variable is
  silently ignored and the daemon falls back to the default path.
- The unit sets `MemoryHigh=32M` instead of the reference's `16M`, with the same
  `MemoryMax=64M`. `ananicy-rs debug memory` reports what your daemon is using.

## Command line and reload

- Use subcommands: `ananicy-rs start` and
  `ananicy-rs dump rules|types|cgroups|proc|autogroup`.
- Service integration is auto-detected; `--systemd` and `--no-systemd` force
  it either way.
- Reload with `SIGUSR1`. It takes effect immediately and rebuilds rules, types
  and cgroups as well as configuration; `check_freq` still needs a restart.
- A few CLI exit codes and diagnostic fields differ from the reference. See
  [CLI](./CLI.md) for the exact behaviour and
  [internals/COMPATIBILITY.md](./internals/COMPATIBILITY.md) for the reasons.

## Configuration and rules

- Existing files work unchanged, including `#` comments and CRLF line endings.
- `apply_ioclass` is accepted but inert, as in the reference; a rule's
  `ioclass` is gated by `apply_ionice`.
- `apply_cpu_weight_from_nice` is new and defaults to the previous behaviour.
  It gates the `nice` → `cpu.weight` mirror described below.
- `check_disks_schedulers` is restored from the original Ananicy and defaults
  to on.
- Booleans must be spelled `true`; `1`, `yes` and `True` evaluate to false.
  `check_freq` must be a positive integer.
- An explicit `null` in a rule deletes an inherited value; a type is merged
  once rather than twice.
- **A type's capacity split can differ.** It uses a floating-point mean, and
  an unreadable CPU counts as missing data rather than zero, so
  `big-cores`/`little-cores` aliases can select different CPUs on a machine
  with an offline CPU or two very different capacities.

## `name_regex`

The engine is PCRE-compatible but not PCRE2. Lookarounds, `$` before a final
newline, leftmost-first alternation and unanchored search are kept. What can
require an edit to a rule:

- `\d`, `\w`, `\D`, `\W` and the POSIX classes are **ASCII-only** now. Under
  `PCRE2_UCP` they were Unicode-aware; use `\p{Nd}`, `\p{L}` and siblings for
  the old meaning.
- Atomic groups, possessive quantifiers, `\K`, branch reset, conditionals and
  a few verbs are refused at load with a logged error. A refused pattern only
  costs the rule its regex matching — exact-name matching still works.
- Pattern size, nesting depth and backtracking are bounded, so a pathological
  pattern can no longer stall start-up.

## Cgroups and process handling

- With the packaged unit, a rule's `cgroup` name resolves **inside** the
  delegated `ananicy-rs.service` subtree; a name beginning with `/` resolves
  from the hierarchy root. A cgroup somebody else manages is refused rather
  than used. See [systemd](./SYSTEMD.md) and
  [Configuration](./CONFIGURATION.md#cgroups-v2-delegation-and-ownership).
- On cgroup v2 an applied `nice` is mirrored into the process' current
  `cpu.weight`, which also reweights other tasks sharing that cgroup. Set
  `apply_cpu_weight_from_nice = false` to keep `nice` only.
- On a hybrid cgroup v1/v2 host this daemon prefers the unified hierarchy;
  the reference stops at the first mount it recognises. A rule's cgroup can
  therefore land in a different directory.
- A deleted executable resolves to its plain name, without the ` (deleted)`
  marker's trailing space, so rules keep matching after a package upgrade.
- Startup waits briefly for cgroup mounts, netlink overruns fall back to a
  full `/proc` scan instead of exiting, and a PID is rechecked before a
  process is moved.
- A process that forks without execing is not reported to the rule engine.
  Its tuning is inherited from the parent, so this only removes a redundant
  re-application.
- NixOS `.foo-wrapped` binaries and `#!` scripts are matched as the program
  rather than the wrapper or interpreter, where the reference matches the
  literal name.
- When one attribute of a rule fails, the rest are still applied and the
  outcome is logged; `dump proc` reports a signed `oom_score_adj` rather than
  wrapping it into an unsigned value.

## Not provided

The reference's developer-only `runqslower_cpp` and `netlink_proc_cpp`
binaries are not built; use `loglevel = trace` instead. Its `crt-static` build
profile has no counterpart.

The remaining, mostly diagnostic differences are recorded in
[internals/COMPATIBILITY.md](./internals/COMPATIBILITY.md).
