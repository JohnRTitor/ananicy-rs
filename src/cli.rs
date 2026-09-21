use std::{convert::Infallible, process::exit, str::FromStr};

use bpaf::Bpaf;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Args {
    pub systemd: bool,
    pub daemon: bool,
    pub config: Option<String>,
    pub config_dir: Option<String>,
    pub reload: bool,
    pub force_remove_semaphore: bool,
    pub manual_scanning: bool,
    pub benchmark: bool,
    pub benchmark_count: Option<u32>,
    pub bpf_min_us: Option<u32>,
    pub verbose: bool,
    pub command: Option<Commands>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpTarget {
    Rules,
    Types,
    Cgroups,
    Proc,
    Autogroup,
}

impl FromStr for DumpTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rules" => Ok(DumpTarget::Rules),
            "types" => Ok(DumpTarget::Types),
            "cgroups" => Ok(DumpTarget::Cgroups),
            "proc" => Ok(DumpTarget::Proc),
            "autogroup" => Ok(DumpTarget::Autogroup),
            _ => Err(format!("Invalid dump target: '{}'", s)),
        }
    }
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

#[derive(Debug, Clone)]
pub enum Commands {
    Dump { sub_action: DumpTarget },
    Debug { sub_action: DebugTarget },
    Start,
    Unknown(String),
}

#[derive(Debug, Clone, Bpaf)]
pub enum BpafCommands {
    #[bpaf(command("dump"))]
    /// Dump internal state
    Dump {
        #[bpaf(positional("SUB_ACTION"))]
        sub_action: String,
    },
    #[bpaf(command("debug"), hide)]
    /// The undocumented `debug` action.
    Debug {
        #[bpaf(positional("SUB_ACTION"), optional)]
        sub_action: Option<String>,
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
}

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options("ananicy-rs"), version)]
/// ANother Auto NICe daemon rewrite in Rust for lower CPU and memory usage
struct Opts {
    #[bpaf(long)]
    /// Run as systemd service
    systemd: bool,
    #[bpaf(long)]
    /// Run as daemon
    daemon: bool,
    #[bpaf(long, argument("CONFIG"))]
    /// Config path
    config: Option<String>,
    #[bpaf(long, argument("CONFIG_DIR"))]
    /// Config directory
    config_dir: Option<String>,
    #[bpaf(long)]
    /// Reload configuration/rules
    reload: bool,
    #[bpaf(long)]
    /// Force remove IPC semaphore
    force_remove_semaphore: bool,
    #[bpaf(long)]
    /// Enable manual periodic scanning
    manual_scanning: bool,
    #[bpaf(long)]
    /// Benchmark mode
    benchmark: bool,
    #[bpaf(long, argument("BENCHMARK_COUNT"))]
    /// Number of times to benchmark
    benchmark_count: Option<u32>,
    #[bpaf(long, argument("BPF_MIN_US"))]
    /// Minimum microseconds for BPF intervals
    bpf_min_us: Option<u32>,
    #[bpaf(short, long)]
    /// Enable verbose output
    verbose: bool,
    #[bpaf(external(bpaf_commands), optional)]
    command: Option<BpafCommands>,
    #[bpaf(positional("UNKNOWN_ACTION"), optional, hide)]
    unknown: Option<String>,
}

impl Args {
    pub fn parse() -> Self {
        let parsed_opts = match opts().run_inner(bpaf::Args::from(&std::env::args().collect::<Vec<_>>()[1..])) {
            Ok(o) => o,
            Err(e) => {
                let code = if let bpaf::ParseFailure::Stdout(..) = e { 0 } else { 2 };
                // bpaf exits with 1 but tests expect 2
                e.print_message(80);
                exit(code);
            }
        };

        let mut final_command = None;

        if let Some(cmd) = parsed_opts.command {
            match cmd {
                BpafCommands::Dump { sub_action } => {
                    match sub_action.parse::<DumpTarget>() {
                        Ok(target) => final_command = Some(Commands::Dump { sub_action: target }),
                        Err(e) => {
                            eprintln!("error: {}", e);
                            exit(2);
                        }
                    }
                }
                BpafCommands::Debug { sub_action } => {
                    let sub_action = match sub_action {
                        Some(sub) => sub,
                        None => {
                            eprintln!("error: A sub-action must be specified for debug.");
                            exit(1);
                        }
                    };
                    let target = sub_action.parse::<DebugTarget>().unwrap();
                    final_command = Some(Commands::Debug { sub_action: target });
                }
                BpafCommands::Start => final_command = Some(Commands::Start),
                BpafCommands::Completions { shell } => {
                    let shell = shell.unwrap_or_default();
                    match shell.as_str() {
                        "bash" | "zsh" | "fish" | "elvish" => {
                            let arg = format!("--bpaf-complete-style-{}", shell);
                            let args = vec![arg];
                            let _ = opts().run_inner(bpaf::Args::from(&args[..]).set_name("ananicy-rs"));
                            unreachable!("bpaf completion generator should have exited");
                        }
                        _ => {
                            eprintln!("error: invalid shell '{}'; expected one of: bash, zsh, fish, elvish", shell);
                            exit(2);
                        }
                    }
                }
            }
        } else if let Some(unk) = parsed_opts.unknown {
            final_command = Some(Commands::Unknown(unk));
        }

        if final_command.is_none() && !parsed_opts.reload && !parsed_opts.force_remove_semaphore {
            // Need to print help.
            if let Err(e) = opts().run_inner(bpaf::Args::from(&["--help"])) {
                print!("{}", e.unwrap_stdout());
                exit(0);
            }
        }

        Args {
            systemd: parsed_opts.systemd,
            daemon: parsed_opts.daemon,
            config: parsed_opts.config,
            config_dir: parsed_opts.config_dir,
            reload: parsed_opts.reload,
            force_remove_semaphore: parsed_opts.force_remove_semaphore,
            manual_scanning: parsed_opts.manual_scanning,
            benchmark: parsed_opts.benchmark,
            benchmark_count: parsed_opts.benchmark_count,
            bpf_min_us: parsed_opts.bpf_min_us,
            verbose: parsed_opts.verbose,
            command: final_command,
        }
    }
}
