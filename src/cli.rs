use {
    nanoargs::{ArgBuilder, Flag, Opt, ParseError, Pos},
    std::env,
};

#[derive(Debug)]
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

impl std::str::FromStr for DumpTarget {
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

/// Parses the raw string provided to `debug [sub_action]` on the CLI.
/// Falls through to a silent success if it doesn't recognize the sub-action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugTarget {
    Cgroups,
    Unknown(String),
}

impl std::str::FromStr for DebugTarget {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "cgroups" => DebugTarget::Cgroups,
            other => DebugTarget::Unknown(other.to_string()),
        })
    }
}

#[derive(Debug)]
pub enum Commands {
    Dump {
        sub_action: DumpTarget,
    },
    /// The undocumented `debug` action.
    ///
    /// Intentionally NOT registered as a formal `nanoargs` subcommand (see
    /// the `NoSubcommand`/`UnknownSubcommand` fallback branch below) so it
    /// [sub-action]` and `start`.
    Debug {
        sub_action: DebugTarget,
    },
    Start,
    Unknown(String),
}

impl Args {
    pub fn parse() -> Self {
        let dump_parser = ArgBuilder::new()
            .name("dump")
            .description("Dump internal state")
            .positional(
                Pos::new("sub_action")
                    .desc("What to dump: rules, types, cgroups, proc, autogroup")
                    .required(),
            )
            .build()
            .unwrap_or_else(|e| {
                eprintln!("internal CLI parser configuration error: {}", e);
                std::process::exit(2);
            });

        let start_parser = ArgBuilder::new()
            .name("start")
            .description("Start the daemon")
            .build()
            .unwrap_or_else(|e| {
                eprintln!("internal CLI parser configuration error: {}", e);
                std::process::exit(2);
            });

        let parser = ArgBuilder::new()
            .name("ananicy-rs")
            .version(env!("CARGO_PKG_VERSION"))
            .description("ANother Auto NICe daemon rewrite in Rust for lower CPU and memory usage")
            .flag(Flag::new("systemd").desc("Run as systemd service"))
            .flag(Flag::new("daemon").desc("Run as daemon"))
            .option(Opt::new("config").desc("Config path").placeholder("CONFIG"))
            .option(
                Opt::new("config-dir")
                    .desc("Config directory")
                    .placeholder("CONFIG_DIR"),
            )
            .flag(Flag::new("reload").desc("Reload configuration/rules"))
            .flag(Flag::new("force-remove-semaphore").desc("Force remove IPC semaphore"))
            .flag(Flag::new("manual-scanning").desc("Enable manual periodic scanning"))
            .flag(Flag::new("benchmark").desc("Benchmark mode"))
            .option(
                Opt::new("benchmark-count")
                    .desc("Number of times to benchmark")
                    .placeholder("BENCHMARK_COUNT"),
            )
            .option(
                Opt::new("bpf-min-us")
                    .desc("Minimum microseconds for BPF intervals")
                    .placeholder("BPF_MIN_US"),
            )
            .flag(
                Flag::new("verbose")
                    .short('v')
                    .desc("Enable verbose output"),
            )
            .subcommand("dump", "Dump internal state", dump_parser)
            .subcommand("start", "Start the daemon", start_parser)
            .build()
            .unwrap_or_else(|e| {
                eprintln!("internal CLI parser configuration error: {}", e);
                std::process::exit(2);
            });

        let args: Vec<String> = env::args().skip(1).collect();
        match parser.parse(args.clone()) {
            Ok(result) => {
                let systemd = result.get_flag("systemd");
                let daemon = result.get_flag("daemon");
                let config = result.get_option("config").map(String::from);
                let config_dir = result.get_option("config-dir").map(String::from);
                let reload = result.get_flag("reload");
                let force_remove_semaphore = result.get_flag("force-remove-semaphore");
                let manual_scanning = result.get_flag("manual-scanning");
                let benchmark = result.get_flag("benchmark");

                let benchmark_count = match result.get_option_parsed::<u32>("benchmark-count") {
                    Some(Ok(v)) => Some(v),
                    Some(Err(e)) => {
                        eprintln!(
                            "error: invalid value for '--benchmark-count <BENCHMARK_COUNT>': {}",
                            e
                        );
                        eprintln!("\nFor more information, try '--help'.");
                        std::process::exit(2);
                    }
                    None => None,
                };

                let bpf_min_us = match result.get_option_parsed::<u32>("bpf-min-us") {
                    Some(Ok(v)) => Some(v),
                    Some(Err(e)) => {
                        eprintln!(
                            "error: invalid value for '--bpf-min-us <BPF_MIN_US>': {}",
                            e
                        );
                        eprintln!("\nFor more information, try '--help'.");
                        std::process::exit(2);
                    }
                    None => None,
                };

                let verbose = result.get_flag("verbose");

                let command = if let Some(subcmd_name) = result.subcommand() {
                    let Some(sub_result) = result.subcommand_result() else {
                        eprintln!(
                            "error: parser returned a subcommand name without its parsed arguments"
                        );
                        std::process::exit(2);
                    };
                    if subcmd_name == "dump" {
                        let sub_action_str = sub_result.get_positionals()[0].to_string();
                        match sub_action_str.parse::<DumpTarget>() {
                            Ok(sub_action) => Some(Commands::Dump { sub_action }),
                            Err(e) => {
                                eprintln!("error: {}", e);
                                std::process::exit(2);
                            }
                        }
                    } else if subcmd_name == "start" {
                        Some(Commands::Start)
                    } else {
                        None
                    }
                } else {
                    None
                };

                if command.is_none() && !reload && !force_remove_semaphore {
                    print!("{}", parser.help_text());
                    std::process::exit(0);
                }

                Args {
                    systemd,
                    daemon,
                    config,
                    config_dir,
                    reload,
                    force_remove_semaphore,
                    manual_scanning,
                    benchmark,
                    benchmark_count,
                    bpf_min_us,
                    verbose,
                    command,
                }
            }
            Err(ParseError::HelpRequested(text)) => {
                // clap prints usage then commands then options
                print!("{}", text);
                std::process::exit(0);
            }
            Err(ParseError::VersionRequested(text)) => {
                println!("{}", text);
                std::process::exit(0);
            }
            Err(ParseError::MissingValue(name)) => {
                let name_upper = name.to_uppercase().replace("-", "_");
                eprintln!(
                    "error: a value is required for '--{} <{}>' but none was supplied",
                    name, name_upper
                );
                eprintln!("\nFor more information, try '--help'.");
                std::process::exit(2);
            }
            Err(ParseError::UnknownArgument(token)) => {
                eprintln!("error: unexpected argument '{}' found", token);
                eprintln!("\nUsage: ananicy-rs [OPTIONS] [COMMAND]");
                eprintln!("\nFor more information, try '--help'.");
                std::process::exit(2);
            }
            Err(ParseError::MissingRequired(name)) => {
                eprintln!(
                    "error: the following required arguments were not provided:\n  <{}>",
                    name.to_uppercase()
                );
                eprintln!("\nUsage: ananicy-rs dump <SUB_ACTION>");
                eprintln!("\nFor more information, try '--help'.");
                std::process::exit(2);
            }
            Err(ParseError::NoSubcommand(_)) | Err(ParseError::UnknownSubcommand(_)) => {
                let help_text = parser.help_text();
                let fallback_parser = ArgBuilder::new()
                    .name("ananicy-rs")
                    .version(env!("CARGO_PKG_VERSION"))
                    .description(
                        "ANother Auto NICe daemon rewrite in Rust for lower CPU and memory usage",
                    )
                    .flag(Flag::new("systemd").desc("Run as systemd service"))
                    .flag(Flag::new("daemon").desc("Run as daemon"))
                    .option(Opt::new("config").desc("Config path").placeholder("CONFIG"))
                    .option(
                        Opt::new("config-dir")
                            .desc("Config directory")
                            .placeholder("CONFIG_DIR"),
                    )
                    .flag(Flag::new("reload").desc("Reload configuration/rules"))
                    .flag(Flag::new("force-remove-semaphore").desc("Force remove IPC semaphore"))
                    .flag(Flag::new("manual-scanning").desc("Enable manual periodic scanning"))
                    .flag(Flag::new("benchmark").desc("Benchmark mode"))
                    .option(
                        Opt::new("benchmark-count")
                            .desc("Number of times to benchmark")
                            .placeholder("BENCHMARK_COUNT"),
                    )
                    .option(
                        Opt::new("bpf-min-us")
                            .desc("Minimum microseconds for BPF intervals")
                            .placeholder("BPF_MIN_US"),
                    )
                    .flag(
                        Flag::new("verbose")
                            .short('v')
                            .desc("Enable verbose output"),
                    )
                    .positional(Pos::new("action").desc("Unknown action fallback"))
                    // Undocumented second positional used only by the `debug` action
                    // (e.g. `debug cgroups`); intentionally not `.required()` so
                    // plain unknown single-word actions (e.g. `nonsense`) keep working.
                    .positional(Pos::new("sub_action").desc("Unknown action fallback"))
                    .build()
                    .unwrap_or_else(|e| {
                        eprintln!("internal CLI parser configuration error: {}", e);
                        std::process::exit(2);
                    });

                match fallback_parser.parse(args) {
                    Ok(result) => {
                        let positionals = result.get_positionals();
                        let command = if positionals.is_empty() {
                            None
                        } else if positionals[0] == "debug" {
                            match positionals.get(1) {
                                None => {
                                    eprintln!("error: A sub-action must be specified for debug.");
                                    std::process::exit(1);
                                }
                                Some(sub) => Some(Commands::Debug {
                                    // infallible: see DebugTarget::from_str
                                    sub_action: sub.parse().unwrap(),
                                }),
                            }
                        } else {
                            Some(Commands::Unknown(positionals[0].to_string()))
                        };

                        let systemd = result.get_flag("systemd");
                        let daemon = result.get_flag("daemon");
                        let config = result.get_option("config").map(String::from);
                        let config_dir = result.get_option("config-dir").map(String::from);
                        let reload = result.get_flag("reload");
                        let force_remove_semaphore = result.get_flag("force-remove-semaphore");
                        let manual_scanning = result.get_flag("manual-scanning");
                        let benchmark = result.get_flag("benchmark");

                        let benchmark_count = match result
                            .get_option_parsed::<u32>("benchmark-count")
                        {
                            Some(Ok(v)) => Some(v),
                            Some(Err(e)) => {
                                eprintln!(
                                    "error: invalid value for '--benchmark-count <BENCHMARK_COUNT>': {}",
                                    e
                                );
                                eprintln!("\nFor more information, try '--help'.");
                                std::process::exit(2);
                            }
                            None => None,
                        };

                        let bpf_min_us = match result.get_option_parsed::<u32>("bpf-min-us") {
                            Some(Ok(v)) => Some(v),
                            Some(Err(e)) => {
                                eprintln!(
                                    "error: invalid value for '--bpf-min-us <BPF_MIN_US>': {}",
                                    e
                                );
                                eprintln!("\nFor more information, try '--help'.");
                                std::process::exit(2);
                            }
                            None => None,
                        };

                        let verbose = result.get_flag("verbose");

                        if command.is_none() && !reload && !force_remove_semaphore {
                            print!("{}", help_text);
                            std::process::exit(0);
                        }

                        Args {
                            systemd,
                            daemon,
                            config,
                            config_dir,
                            reload,
                            force_remove_semaphore,
                            manual_scanning,
                            benchmark,
                            benchmark_count,
                            bpf_min_us,
                            verbose,
                            command,
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {}", e);
                        std::process::exit(2);
                    }
                }
            }
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(2);
            }
        }
    }
}
