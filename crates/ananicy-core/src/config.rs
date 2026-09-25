use std::fmt::{Display, Formatter, Result};

use {
    std::{fs, io, path::Path, sync::Arc},
    tracing::{error, info, warn},
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
    Critical,
}

impl LogLevel {
    fn parse_with_status(s: &str) -> (Self, bool) {
        match s.to_lowercase().as_str() {
            "trace" => (LogLevel::Trace, true),
            "debug" => (LogLevel::Debug, true),
            "info" => (LogLevel::Info, true),
            "warn" => (LogLevel::Warn, true),
            "error" => (LogLevel::Error, true),
            "critical" | "fatal" => (LogLevel::Critical, true),
            _ => (LogLevel::Info, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigDiagnostic {
    Info(String),
    Warn(String),
    Error(String),
}

impl ConfigDiagnostic {
    pub fn emit(&self) {
        match self {
            Self::Info(message) => info!("{}", message),
            Self::Warn(message) => warn!("{}", message),
            Self::Error(message) => error!("{}", message),
        }
    }
}

impl From<&LogLevel> for tracing::Level {
    fn from(val: &LogLevel) -> Self {
        match val {
            LogLevel::Trace => tracing::Level::TRACE,
            LogLevel::Debug => tracing::Level::DEBUG,
            LogLevel::Info => tracing::Level::INFO,
            LogLevel::Warn => tracing::Level::WARN,
            LogLevel::Error | LogLevel::Critical => tracing::Level::ERROR,
        }
    }
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            LogLevel::Trace => write!(f, "trace"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
            LogLevel::Critical => write!(f, "critical"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub check_freq: u32,
    pub cgroup_load: bool,
    pub type_load: bool,
    pub rule_load: bool,
    pub apply_nice: bool,
    pub apply_latnice: bool,
    pub apply_ioclass: bool,
    pub apply_ionice: bool,
    pub apply_sched: bool,
    pub apply_oom_score_adj: bool,
    pub apply_cgroups: bool,
    pub apply_cpuset: bool,
    pub cgroup_realtime_workaround: bool,
    pub x3d_mode: String,
    pub log_applied_rule: bool,
    pub loglevel: LogLevel,
}

impl Default for ConfigSnapshot {
    fn default() -> Self {
        Self {
            check_freq: 60,
            cgroup_load: true,
            type_load: true,
            rule_load: true,
            apply_nice: true,
            apply_latnice: true,
            apply_ioclass: true,
            apply_ionice: true,
            apply_sched: true,
            apply_oom_score_adj: true,
            apply_cgroups: true,
            apply_cpuset: true,
            cgroup_realtime_workaround: true,
            x3d_mode: "auto".to_string(),
            log_applied_rule: false,
            loglevel: LogLevel::default(),
        }
    }
}

impl ConfigSnapshot {
    pub fn parse_file<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let (snapshot, diagnostics) = Self::parse_file_with_diagnostics(path)?;
        for diagnostic in &diagnostics {
            diagnostic.emit();
        }
        Ok(snapshot)
    }

    pub fn parse_file_with_diagnostics<P: AsRef<Path>>(
        path: P,
    ) -> io::Result<(Self, Vec<ConfigDiagnostic>)> {
        let content = fs::read_to_string(&path)?;
        let mut config = Self::default();
        let mut diagnostics = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();

                match key {
                    "check_freq" => {
                        if let Ok(freq) = value.parse() {
                            config.check_freq = freq;
                        } else {
                            diagnostics.push(ConfigDiagnostic::Error(format!(
                                "Invalid check_freq value: {}",
                                value
                            )));
                        }
                    }
                    "cgroup_load" => config.cgroup_load = value == "true",
                    "type_load" => config.type_load = value == "true",
                    "rule_load" => config.rule_load = value == "true",
                    "apply_nice" => config.apply_nice = value == "true",
                    "apply_latnice" => config.apply_latnice = value == "true",
                    "apply_ioclass" => config.apply_ioclass = value == "true",
                    "apply_ionice" => config.apply_ionice = value == "true",
                    "apply_sched" => config.apply_sched = value == "true",
                    "apply_oom_score_adj" => config.apply_oom_score_adj = value == "true",
                    "apply_cgroup" => config.apply_cgroups = value == "true",
                    "apply_cpuset" => config.apply_cpuset = value == "true",
                    "cgroup_realtime_workaround" => {
                        config.cgroup_realtime_workaround = value == "true"
                    }
                    "x3d_mode" => config.x3d_mode = value.to_string(),
                    "log_applied_rule" => config.log_applied_rule = value == "true",
                    "loglevel" => {
                        let (level, valid) = LogLevel::parse_with_status(value);
                        config.loglevel = level;
                        if !valid {
                            diagnostics.push(ConfigDiagnostic::Warn(format!(
                                "Unknown loglevel '{}', falling back to info",
                                value
                            )));
                        }
                    }
                    _ => diagnostics.push(ConfigDiagnostic::Warn(format!(
                        "Unknown config key: {}",
                        key
                    ))),
                }
            }
        }

        Ok((config, diagnostics))
    }

    pub fn to_config_string(&self) -> String {
        format!(
            "# Generated by ananicy-rs\n\
            apply_nice={}\n\
            apply_sched={}\n\
            apply_ionice={}\n\
            apply_oom_score_adj={}\n\
            apply_latnice={}\n\
            log_applied_rule={}\n\
            cgroup_load={}\n\
            type_load={}\n\
            rule_load={}\n\
            cgroup_realtime_workaround={}\n\
            apply_cgroup={}\n\
            apply_cpuset={}\n\
            x3d_mode={}\n\
            loglevel={}\n\
            check_freq={}\n",
            self.apply_nice,
            self.apply_sched,
            self.apply_ionice,
            self.apply_oom_score_adj,
            self.apply_latnice,
            self.log_applied_rule,
            self.cgroup_load,
            self.type_load,
            self.rule_load,
            self.cgroup_realtime_workaround,
            self.apply_cgroups,
            self.apply_cpuset,
            self.x3d_mode,
            self.loglevel,
            self.check_freq
        )
    }
}

/// Thread-safe configuration manager
pub struct Config {
    snapshot: arc_swap::ArcSwap<ConfigSnapshot>,
}

impl Config {
    pub fn new(snapshot: ConfigSnapshot) -> Self {
        Self {
            snapshot: arc_swap::ArcSwap::from_pointee(snapshot),
        }
    }

    pub fn load_file<P: AsRef<Path>>(path: P, latnice_supported: bool) -> io::Result<Self> {
        let (config, diagnostics) = Self::load_file_with_diagnostics(path, latnice_supported)?;
        for diagnostic in &diagnostics {
            diagnostic.emit();
        }
        Ok(config)
    }

    pub fn load_file_with_diagnostics<P: AsRef<Path>>(
        path: P,
        latnice_supported: bool,
    ) -> io::Result<(Self, Vec<ConfigDiagnostic>)> {
        let path = path.as_ref();
        let mut diagnostics = Vec::new();
        let mut snapshot = match ConfigSnapshot::parse_file_with_diagnostics(path) {
            Ok((snapshot, parse_diagnostics)) => {
                diagnostics.extend(parse_diagnostics);
                snapshot
            }
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                diagnostics.push(ConfigDiagnostic::Info(format!(
                    "Configuration file {} does not exist; using defaults",
                    path.display()
                )));
                let mut snapshot = ConfigSnapshot::default();
                if !latnice_supported {
                    snapshot.apply_latnice = false;
                }

                let config_string = snapshot.to_config_string();
                diagnostics.push(ConfigDiagnostic::Info(format!(
                    "Default config:\n{}",
                    config_string
                )));
                diagnostics.push(ConfigDiagnostic::Info(format!(
                    "Writing default config to {}",
                    path.display()
                )));

                if let Some(parent) = path.parent()
                    && !parent.exists()
                    && let Err(create_err) = fs::create_dir_all(parent)
                {
                    diagnostics.push(ConfigDiagnostic::Error(format!(
                        "Cannot create config directory {}: {}",
                        parent.display(),
                        create_err
                    )));
                }

                if let Err(write_err) = fs::write(path, config_string) {
                    diagnostics.push(ConfigDiagnostic::Error(format!(
                        "Cannot write config to {}: {}",
                        path.display(),
                        write_err
                    )));
                }
                snapshot
            }
            Err(e) => return Err(e),
        };

        if !latnice_supported {
            snapshot.apply_latnice = false;
        }
        Ok((Self::new(snapshot), diagnostics))
    }

    pub fn get(&self) -> arc_swap::Guard<Arc<ConfigSnapshot>> {
        self.snapshot.load()
    }

    pub fn reload_file<P: AsRef<Path>>(&self, path: P, latnice_supported: bool) -> io::Result<()> {
        let diagnostics = self.reload_file_with_diagnostics(path, latnice_supported)?;
        for diagnostic in &diagnostics {
            diagnostic.emit();
        }
        Ok(())
    }

    pub fn reload_file_with_diagnostics<P: AsRef<Path>>(
        &self,
        path: P,
        latnice_supported: bool,
    ) -> io::Result<Vec<ConfigDiagnostic>> {
        let (mut snapshot, diagnostics) = ConfigSnapshot::parse_file_with_diagnostics(path)?;
        if !latnice_supported {
            snapshot.apply_latnice = false;
        }
        self.snapshot.store(Arc::new(snapshot));
        Ok(diagnostics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_enables_the_whole_daemon() {
        let config = ConfigSnapshot::default();
        assert_eq!(config.check_freq, 60);
        assert_eq!(config.x3d_mode, "auto");
        assert!(config.apply_nice);
        assert!(config.apply_latnice);
        assert!(config.apply_sched);
        assert!(config.apply_ioclass);
        assert!(config.apply_ionice);
        assert!(config.apply_oom_score_adj);
        assert!(config.apply_cgroups);
        assert!(config.apply_cpuset);
        assert!(config.cgroup_load);
        assert!(config.type_load);
        assert!(config.rule_load);
        assert!(config.cgroup_realtime_workaround);
        assert!(!config.log_applied_rule);
        assert_eq!(config.loglevel, LogLevel::Info);
    }

    #[test]
    fn load_writes_defaults_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.conf");
        let _config = Config::load_file(&path, true).unwrap();
        assert!(
            path.exists(),
            "load_file should write default config if it doesn't exist"
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("cgroup_realtime_workaround=true"));
    }

    #[test]
    fn load_writes_defaults_into_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/ananicy.conf");

        Config::load_file(&path, true).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn generated_defaults_honour_an_unsupported_latnice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.conf");

        let (config, diagnostics) = Config::load_file_with_diagnostics(&path, false).unwrap();

        assert!(!config.get().apply_latnice);
        assert!(
            config.get().apply_nice,
            "the other flags keep their default"
        );
        assert!(
            diagnostics.iter().any(|d| matches!(
                d,
                ConfigDiagnostic::Info(message) if message.contains("does not exist")
            )),
            "falling back to defaults is reported: {diagnostics:?}"
        );
        let written = fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("apply_latnice=false"),
            "the generated file must not request an unsupported attribute: {written}"
        );
    }

    #[test]
    fn reload_disables_latnice_if_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reload.conf");
        fs::write(&path, "apply_latnice=true\n").unwrap();

        let config = Config::load_file(&path, true).unwrap();
        assert!(config.get().apply_latnice);

        // Reload but simulate kernel not supporting latnice anymore
        config.reload_file(&path, false).unwrap();
        assert!(
            !config.get().apply_latnice,
            "Reload should disable apply_latnice if unsupported"
        );
    }

    #[test]
    fn loglevel_parsing_is_case_insensitive_and_knows_legacy_aliases() {
        for (input, expected) in [
            ("trace", LogLevel::Trace),
            ("DEBUG", LogLevel::Debug),
            ("Info", LogLevel::Info),
            ("warn", LogLevel::Warn),
            ("error", LogLevel::Error),
            ("critical", LogLevel::Critical),
            ("CRITICAL", LogLevel::Critical),
            ("fatal", LogLevel::Critical),
            ("FATAL", LogLevel::Critical),
        ] {
            assert_eq!(LogLevel::parse_with_status(input), (expected, true));
        }
    }

    #[test]
    fn unknown_loglevel_falls_back_to_info_and_is_reported() {
        assert_eq!(
            LogLevel::parse_with_status("unknown_level"),
            (LogLevel::Info, false)
        );
        assert_eq!(
            LogLevel::parse_with_status(""),
            (LogLevel::Info, false),
            "an empty value is not a valid level"
        );
    }

    #[test]
    fn loglevel_display_round_trips_through_the_parser() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
            LogLevel::Critical,
        ] {
            let (parsed, valid) = LogLevel::parse_with_status(&level.to_string());
            assert!(valid, "{level} must be parseable");
            assert_eq!(parsed, level);
        }
    }
}
