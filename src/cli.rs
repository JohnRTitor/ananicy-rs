use std::{convert::Infallible, process::exit, str::FromStr};

use bpaf::{Bpaf, ShellComp};

use crate::systemd::SystemdRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpTarget {
    Rules,
    Types,
    Cgroups,
    Proc,
    Autogroup,
}

const DUMP_TARGET_NAMES: [&str; 5] = ["rules", "types", "cgroups", "proc", "autogroup"];

const DEBUG_TARGET_NAMES: [&str; 1] = ["cgroups"];

impl FromStr for DumpTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rules" => Ok(DumpTarget::Rules),
            "types" => Ok(DumpTarget::Types),
            "cgroups" => Ok(DumpTarget::Cgroups),
            "proc" => Ok(DumpTarget::Proc),
            "autogroup" => Ok(DumpTarget::Autogroup),
            _ => Err(format!(
                "Invalid dump target: '{}'; expected one of: {}",
                s,
                DUMP_TARGET_NAMES.join(", ")
            )),
        }
    }
}

fn complete_from(
    names: &'static [&'static str],
    input: &str,
) -> Vec<(&'static str, Option<&'static str>)> {
    names
        .iter()
        .filter(|name| name.starts_with(input))
        .map(|name| (*name, None))
        .collect()
}

// bpaf's `complete` takes `&T` where `T` is the parsed type, so `&String` is
// required here: each sub-action is parsed *after* it has been completed, so
// that a prefix that is not a target yet ("cg") still completes.
#[allow(clippy::ptr_arg)]
fn dump_completer(input: &String) -> Vec<(&'static str, Option<&'static str>)> {
    complete_from(&DUMP_TARGET_NAMES, input)
}

fn debug_completer(input: &Option<String>) -> Vec<(&'static str, Option<&'static str>)> {
    complete_from(&DEBUG_TARGET_NAMES, input.as_deref().unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugTarget {
    Cgroups,
    Unknown(String),
}

impl FromStr for DebugTarget {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "cgroups" => DebugTarget::Cgroups,
            other => DebugTarget::Unknown(other.to_string()),
        })
    }
}

/// bpaf hands the `dump` sub-action over as a string, and an unknown target is
/// a usage error, so this one can fail.
fn dump_target(raw: String) -> Result<DumpTarget, String> {
    raw.parse()
}

/// The `debug` sub-action is optional, and every string is a valid one, so
/// nothing can fail here.
fn debug_target(raw: Option<String>) -> Option<DebugTarget> {
    raw.map(|raw| DebugTarget::from_str(&raw).expect("`DebugTarget` accepts any string"))
}

// A variant doc comment is the action's help text, so the notes that are not
// help are `//` comments.
#[derive(Debug, Clone, Bpaf)]
pub enum Commands {
    #[bpaf(command("dump"))]
    /// Dump internal state
    Dump {
        #[bpaf(
            positional::<String>("SUB_ACTION"),
            complete(dump_completer),
            complete_shell(ShellComp::Nothing),
            parse(dump_target)
        )]
        sub_action: DumpTarget,
    },
    #[bpaf(command("debug"), hide)]
    /// The undocumented `debug` action.
    Debug {
        #[bpaf(
            positional::<String>("SUB_ACTION"),
            optional,
            complete(debug_completer),
            complete_shell(ShellComp::Nothing),
            map(debug_target)
        )]
        sub_action: Option<DebugTarget>,
    },
    #[bpaf(command("start"))]
    /// Start the daemon
    Start,
    #[bpaf(command("completions"))]
    /// Generate shell completions
    Completions {
        #[bpaf(positional("SHELL"), optional)]
        shell: Option<String>,
    },
    // Not an action at all: the reference reported an unrecognised action and
    // exited 1 rather than refusing the command line, so `Args::parse` builds
    // this from the leftover positional and bpaf never sees it. `skip` keeps it
    // out of the alternatives, which is what leaves an action name that fails
    // to parse (`dump` with no sub-action) to report its own error instead of
    // being read as one.
    #[bpaf(skip)]
    Unknown(String),
}

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options("ananicy-rs"), version)]
/// ANother Auto NICe daemon rewrite in Rust for lower CPU and memory usage
pub struct Args {
    #[bpaf(long)]
    /// Run as systemd service (detected automatically when omitted)
    systemd: bool,
    #[bpaf(long)]
    /// Never use systemd integration, even when auto-detected
    no_systemd: bool,
    #[bpaf(long)]
    /// Run as daemon
    pub daemon: bool,
    #[bpaf(long, argument("CONFIG"))]
    /// Config path
    pub config: Option<String>,
    #[bpaf(long, argument("CONFIG_DIR"))]
    /// Config directory
    pub config_dir: Option<String>,
    #[bpaf(long)]
    /// Reload the configuration, the log level, and the rule, type and cgroup
    /// files. Processes already tuned keep their settings until they are seen
    /// again.
    pub reload: bool,
    #[bpaf(long)]
    /// Force remove IPC semaphore
    pub force_remove_semaphore: bool,
    // Both spellings: `--manualscanning` is what the Ananicy command line this
    // daemon is compatible with used, and ananicy-cpp still accepts it.
    #[bpaf(long("manual-scanning"), long("manualscanning"))]
    /// Enable manual periodic scanning (also accepted as `--manualscanning`)
    pub manual_scanning: bool,
    #[bpaf(long)]
    /// Benchmark mode
    pub benchmark: bool,
    #[bpaf(long, argument("BENCHMARK_COUNT"))]
    /// Number of times to benchmark
    pub benchmark_count: Option<u32>,
    #[bpaf(long, argument("BPF_MIN_US"))]
    /// Minimum microseconds for BPF intervals
    pub bpf_min_us: Option<u32>,
    #[bpaf(short, long)]
    /// Enable verbose output
    pub verbose: bool,
    #[bpaf(external(commands), optional)]
    pub command: Option<Commands>,
    // Folded into `command` by `parse`, so that an unrecognised action is
    // reported rather than refused. This is a positional, and positionals come
    // last.
    #[bpaf(positional("UNKNOWN_ACTION"), optional, hide)]
    unknown: Option<String>,
}

impl Args {
    /// What the systemd flags asked for, refusing the pair that asks for both
    /// directions at once.
    ///
    /// Refused by this and not by the caller, because the pair is a usage error
    /// and a usage error has to be settled before the no-command fallback
    /// below answers the run with the help text instead.
    pub fn systemd_request(&self) -> SystemdRequest {
        match (self.systemd, self.no_systemd) {
            (true, true) => {
                eprintln!("error: --systemd and --no-systemd are mutually exclusive");
                exit(2);
            }
            (true, false) => SystemdRequest::Enabled,
            (false, true) => SystemdRequest::Disabled,
            (false, false) => SystemdRequest::Auto,
        }
    }

    pub fn parse() -> Self {
        let mut parsed =
            match args().run_inner(bpaf::Args::from(&std::env::args().collect::<Vec<_>>()[1..])) {
                Ok(o) => o,
                Err(e) => {
                    let code = if let bpaf::ParseFailure::Stdout(..) = e {
                        0
                    } else {
                        2
                    };
                    // bpaf exits with 1 but tests expect 2
                    e.print_message(80);
                    exit(code);
                }
            };

        // Settled here, before the no-command fallback at the bottom of this
        // function, which would otherwise answer a bare `--systemd
        // --no-systemd` with the help text.
        let _ = parsed.systemd_request();

        if let Some(Commands::Completions { shell }) = &parsed.command {
            let shell = shell.clone().unwrap_or_default();
            match shell.as_str() {
                "bash" | "zsh" | "fish" | "elvish" => {
                    let arg = format!("--bpaf-complete-style-{}", shell);
                    let arg = [arg];
                    let _ = args().run_inner(bpaf::Args::from(&arg[..]).set_name("ananicy-rs"));
                    unreachable!("bpaf completion generator should have exited");
                }
                _ => {
                    eprintln!(
                        "error: invalid shell '{}'; expected one of: bash, zsh, fish, elvish",
                        shell
                    );
                    exit(2);
                }
            }
        }

        // Refused here rather than by the parser, because a `debug` with no
        // sub-action is not a usage error and must not be reported as one: it
        // exits 1, and it does so before anything is read or written.
        if matches!(parsed.command, Some(Commands::Debug { sub_action: None })) {
            eprintln!("error: A sub-action must be specified for debug.");
            exit(1);
        }

        if parsed.command.is_none()
            && let Some(unknown) = parsed.unknown.take()
        {
            parsed.command = Some(Commands::Unknown(unknown));
        }

        if parsed.command.is_none() && !parsed.reload && !parsed.force_remove_semaphore {
            // Need to print help.
            if let Err(e) = args().run_inner(bpaf::Args::from(&["--help"])) {
                print!("{}", e.unwrap_stdout());
                exit(0);
            }
        }

        parsed
    }
}
