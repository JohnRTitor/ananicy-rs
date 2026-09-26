//! The I/O schedulers the block devices on this machine are using.
//!
//! `ioprio_set(2)` — and therefore a rule's `ioclass` and `ionice` — is honoured
//! only by the schedulers the kernel says it is: `bfq` and `mq-deadline`, per
//! `Documentation/block/ioprio.rst`. `cfq` went with the legacy single-queue
//! block layer, so it is only ever the active scheduler on an old kernel and is
//! kept for those. On anything else the call succeeds and the value goes
//! nowhere, which is the worst kind of failure: the daemon reports that it
//! applied a rule, and the I/O priority is unchanged.
//!
//! The original Ananicy checked for this at start-up and printed one line per
//! disk. That check was dropped when the daemon was rewritten — it is in the
//! original's shipped `ananicy.conf` (`check_disks_schedulers=true`) and in
//! neither rewrite's configuration — so it is restored here. The check is
//! read-only, cheap, and only reports.

use {
    std::{fs, path::Path},
    tracing::warn,
};

/// The schedulers that honour `ioprio_set(2)`: `bfq` and `mq-deadline`, plus
/// `cfq` for kernels old enough to still have it.
const SUPPORTED: [&str; 4] = ["mq-deadline", "bfq", "bfq-mq", "cfq"];

/// Devices whose names contain these are not worth reporting: loop and ram
/// devices have no I/O scheduler a user cares about, and `sr` is optical media.
const IGNORED: [&str; 3] = ["loop", "ram", "sr"];

/// The scheduler a device is using, from the contents of its `queue/scheduler`.
///
/// The kernel lists every available scheduler with the active one in brackets —
/// `[none] mq-deadline kyber bfq` — so the answer is what is inside them. A
/// file that is empty or unbracketed has no scheduler, which is reported as such
/// rather than guessed at.
fn active_scheduler(listing: &str) -> Option<&str> {
    let (_, rest) = listing.split_once('[')?;
    let (active, _) = rest.split_once(']')?;
    Some(active)
}

/// Reports every block device whose scheduler will not honour `ioclass`.
///
/// Returns how many were reported, which is what the tests assert on; the
/// messages themselves go to the log. A machine without `/sys/class/block` — a
/// container, a kernel built without it — is not an error, and is silent.
pub(crate) fn check_disk_schedulers(block_class: &Path) -> usize {
    let Ok(devices) = fs::read_dir(block_class) else {
        return 0;
    };

    let mut reported = 0;
    for device in devices.flatten() {
        let name = device.file_name();
        let name = name.to_string_lossy();
        if IGNORED.iter().any(|skip| name.contains(skip)) {
            continue;
        }

        let listing = match fs::read_to_string(device.path().join("queue/scheduler")) {
            Ok(listing) => listing,
            // No scheduler file: not a queue we can say anything about.
            Err(_) => continue,
        };

        let active = active_scheduler(&listing).unwrap_or("").trim();
        if SUPPORTED.contains(&active) {
            continue;
        }

        warn!(
            "Disk {name} is on a scheduler that does not honour ioprio (it is using \
             {active:?}), so ioclass and ionice will not work for it"
        );
        reported += 1;
    }

    reported
}

/// Whether the check is wanted, per the configuration.
pub(crate) fn check_disk_schedulers_if_enabled(enabled: bool) -> usize {
    if !enabled {
        return 0;
    }
    check_disk_schedulers(Path::new("/sys/class/block"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block-device tree: one directory per device, with a `queue/scheduler`
    /// file listing the available schedulers and the active one bracketed.
    fn block_class(disks: &[(&str, Option<&str>)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a temporary /sys/class/block");
        for (name, scheduler) in disks {
            let queue = root.path().join(name).join("queue");
            fs::create_dir_all(&queue).expect("a device directory");
            if let Some(scheduler) = scheduler {
                fs::write(queue.join("scheduler"), format!("{scheduler}\n"))
                    .expect("a scheduler file");
            }
        }
        root
    }

    #[test]
    fn the_active_scheduler_is_the_bracketed_one() {
        assert_eq!(
            active_scheduler("[none] mq-deadline kyber bfq"),
            Some("none")
        );
        assert_eq!(active_scheduler("noop [bfq]"), Some("bfq"));
        assert_eq!(active_scheduler("  [cfq]  "), Some("cfq"));
        assert_eq!(active_scheduler(""), None);
        assert_eq!(active_scheduler("no brackets here"), None);
    }

    #[test]
    fn a_device_on_a_supported_scheduler_is_not_reported() {
        let root = block_class(&[
            ("sda", Some("[bfq] mq-deadline")),
            ("sdb", Some("[cfq]")),
            ("sdc", Some("[mq-deadline] none kyber bfq")),
        ]);
        assert_eq!(check_disk_schedulers(root.path()), 0);
    }

    #[test]
    fn an_nvme_device_on_mq_deadline_is_not_reported() {
        // What every NVMe machine since ~4.12 looks like. The kernel honours
        // ioprio on mq-deadline, so warning about it is a false positive.
        let root = block_class(&[
            ("nvme0n1", Some("[mq-deadline] none kyber bfq")),
            ("nvme1n1", Some("[mq-deadline] none kyber bfq")),
        ]);
        assert_eq!(check_disk_schedulers(root.path()), 0);
    }

    #[test]
    fn a_device_on_anything_else_is_reported() {
        let root = block_class(&[
            ("sda", Some("[none] mq-deadline")),
            ("sdb", Some("[bfq]")),
            ("sdc", Some("[mq-deadline] none kyber bfq")),
            ("nvme0n1", Some("[none] kyber")),
        ]);
        assert_eq!(
            check_disk_schedulers(root.path()),
            2,
            "the bfq and mq-deadline devices are fine, `none` and kyber are not"
        );
    }

    #[test]
    fn devices_that_have_no_useful_scheduler_are_skipped() {
        let root = block_class(&[
            ("loop0", Some("[none]")),
            ("ram0", Some("[none]")),
            ("sr0", Some("[none]")),
            // A device with no scheduler file at all: nothing to say about it.
            ("dm-0", None),
        ]);
        assert_eq!(check_disk_schedulers(root.path()), 0);
    }

    #[test]
    fn a_missing_block_class_is_silent() {
        // A container, or a kernel without the block layer, is not an error.
        let root = tempfile::tempdir().expect("an empty directory");
        assert_eq!(check_disk_schedulers(&root.path().join("block")), 0);
    }

    #[test]
    fn the_check_can_be_switched_off() {
        // The flag decides whether the scan happens at all, so nothing is read
        // and nothing is reported.
        assert_eq!(check_disk_schedulers_if_enabled(false), 0);
    }
}
