//! Machine-wide daemon instance slots.
//!
//! Each running daemon holds an exclusive `flock` on one numbered slot file,
//! `leanck-slot-{N}.lock`, in the runtime directory for its whole life and
//! records who it is in that file as one JSON line. The OS releases the lock
//! when the descriptor closes, including on crash, so a slot never wedges.
//!
//! Acquisition holds the admission lock `leanck-slots.lock` in the same
//! directory while it takes a slot and counts running daemons by testing the
//! other slot files' locks, so the slot index is not the count. Counting and
//! listing ([`scan`], [`live`]) only read slot files; they never take a lock.
//! A record is live only while its pid is alive and that process has the
//! recorded start identity ([`process_start_id`]), so a crashed daemon's record
//! does not come back to life when its pid is reused.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{File, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `EX_TEMPFAIL`: the daemon exit status when it refuses to start over the instance limit.
pub const EXIT_INSTANCE_LIMIT: i32 = 75;
/// Scan ceiling for slot acquisition. Configuration rejects limits above
/// `leanck_config::Instances::MAX_LIMIT`, which `leanckd` asserts equals this.
pub const MAX_SLOTS: u32 = 4096;

const SLOT_PREFIX: &str = "leanck-slot-";
const SLOT_SUFFIX: &str = ".lock";
/// Serializes acquisition; [`parse_slot_name`] never matches it.
const ADMISSION_LOCK: &str = "leanck-slots.lock";
/// Seconds a record's process may appear to have started after `started_at`,
/// for records without a start identity. The daemon writes `started_at` after
/// its own process starts, so only clock granularity separates the two.
const START_SLACK: u64 = 5;

/// `directory/leanck-slot-{slot}.lock`.
pub fn slot_path(directory: &Path, slot: u32) -> PathBuf {
    directory.join(format!("{SLOT_PREFIX}{slot}{SLOT_SUFFIX}"))
}

/// Inverse of [`slot_path`]'s file name; only canonical decimal numbers are accepted.
pub fn parse_slot_name(file_name: &OsStr) -> Option<u32> {
    let digits = file_name
        .to_str()?
        .strip_prefix(SLOT_PREFIX)?
        .strip_suffix(SLOT_SUFFIX)?;
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse().ok()
}

/// The identity a daemon writes into its slot file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotRecord {
    pub pid: u32,
    /// Canonical worktree root.
    pub worktree: PathBuf,
    pub socket: PathBuf,
    /// Unix seconds.
    pub started_at: u64,
    /// The process's [`process_start_id`]. Unlike `started_at`, a wall-clock
    /// step never changes it. Absent where the platform has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_id: Option<u64>,
}

/// How acquisition treats a full set of slots below the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotPolicy {
    /// Stop at the limit and report [`SlotOutcome::OverLimit`].
    Refuse,
    /// Keep scanning past the limit (configuration `warn`, and limit 0).
    Continue,
}

/// An exclusively held slot. The lock lasts until this value is dropped.
#[derive(Debug)]
pub struct SlotLock {
    slot: u32,
    path: PathBuf,
    file: File,
}

#[derive(Debug)]
pub enum SlotOutcome {
    /// `running` counts held slots, this daemon's included.
    Acquired { lock: SlotLock, running: u64 },
    /// `held` slots were held by other daemons, reaching `limit`.
    OverLimit { held: u64, limit: u64 },
}

impl SlotLock {
    /// Lock the lowest free slot and write `record` into it as one JSON line.
    ///
    /// The whole call holds the admission lock, a blocking exclusive lock on
    /// `leanck-slots.lock`, released on every return. Slot files are opened
    /// without truncation and tried in order 0, 1, 2, …. `held` counts the
    /// other existing slot files whose lock is held; a free one is released at
    /// once and not counted. With [`SlotPolicy::Refuse`] and a limit `N > 0`,
    /// returns `OverLimit { held, limit: N }` when `held >= N`, leaving no record
    /// and creating no slot file. A limit of 0 means unlimited. Lock errors
    /// other than contention propagate, as does exhausting all [`MAX_SLOTS`] slots.
    pub fn acquire(
        directory: &Path,
        limit: u64,
        policy: SlotPolicy,
        record: &SlotRecord,
    ) -> io::Result<SlotOutcome> {
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        line.push(b'\n');
        // Dropping this descriptor on any return releases admission.
        let admission = open_for_locking(&directory.join(ADMISSION_LOCK))?;
        admission.lock()?;
        let existing = existing_slots(directory)?;
        let refuse = policy == SlotPolicy::Refuse && limit > 0;
        for slot in 0..MAX_SLOTS {
            // The loop only advances past held slots, so every lower slot is held.
            let below = u64::from(slot);
            let mut above = None;
            if refuse && !existing.contains(&slot) {
                // Taking this slot would create its file; refuse first if full.
                let counted = count_held(directory, existing.range(slot + 1..))?;
                if below + counted >= limit {
                    return Ok(SlotOutcome::OverLimit {
                        held: below + counted,
                        limit,
                    });
                }
                above = Some(counted);
            }
            let path = slot_path(directory, slot);
            let mut file = open_for_locking(&path)?;
            match file.try_lock() {
                Ok(()) => {
                    let held = below
                        + match above {
                            Some(counted) => counted,
                            None => count_held(directory, existing.range(slot + 1..))?,
                        };
                    file.set_len(0)?;
                    if refuse && held >= limit {
                        return Ok(SlotOutcome::OverLimit { held, limit });
                    }
                    file.write_all(&line)?;
                    file.flush()?;
                    return Ok(SlotOutcome::Acquired {
                        lock: SlotLock { slot, path, file },
                        running: held + 1,
                    });
                }
                Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Error(error)) => return Err(error),
            }
        }
        Err(io::Error::other(format!(
            "all {MAX_SLOTS} leanck instance slots in {} are held",
            directory.display()
        )))
    }

    pub fn slot(&self) -> u32 {
        self.slot
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Open (creating if needed, never truncating) a file to `flock`.
fn open_for_locking(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

/// The slot numbers of the slot files currently in `directory`.
fn existing_slots(directory: &Path) -> io::Result<BTreeSet<u32>> {
    let mut slots = BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        if let Some(slot) = parse_slot_name(&entry?.file_name()) {
            slots.insert(slot);
        }
    }
    Ok(slots)
}

/// How many of `slots` another holder has locked. A slot found free is
/// released at once; a missing file is skipped and never created.
fn count_held<'a>(directory: &Path, slots: impl IntoIterator<Item = &'a u32>) -> io::Result<u64> {
    let mut held = 0;
    for &slot in slots {
        let file = match File::open(slot_path(directory, slot)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => held += 1,
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(held)
}

impl Drop for SlotLock {
    fn drop(&mut self) {
        // Clear the record while still holding the lock so a recycled pid
        // cannot make a released slot look live. The file itself stays.
        let _ = self.file.set_len(0);
    }
}

/// One readable slot record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotEntry {
    pub slot: u32,
    pub record: SlotRecord,
}

/// Read every slot record in `directory`, sorted by slot.
///
/// Read-only: never locks, never creates the directory. A missing directory
/// yields no entries; empty and malformed files are skipped.
pub fn scan(directory: &Path) -> Vec<SlotEntry> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut slots: Vec<SlotEntry> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let slot = parse_slot_name(&entry.file_name())?;
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let record = serde_json::from_str(text.lines().next()?.trim()).ok()?;
            Some(SlotEntry { slot, record })
        })
        .collect();
    slots.sort_by_key(|entry| entry.slot);
    slots
}

/// [`scan`] filtered to live records (see [`record_is_live`]).
pub fn live(directory: &Path) -> Vec<SlotEntry> {
    scan(directory)
        .into_iter()
        .filter(|entry| record_is_live(&entry.record))
        .collect()
}

/// Whether `record` still describes a running process: its pid is alive and
/// that process is the one that wrote the record. When the record and the
/// platform both know a [`process_start_id`], they must be equal. Otherwise,
/// when the process start time is known, it must be no later than `started_at`
/// plus a few seconds of slack. A process that fails either check reuses the
/// pid of a daemon that died without clearing its record.
pub fn record_is_live(record: &SlotRecord) -> bool {
    if !pid_alive(record.pid) {
        return false;
    }
    match (record.start_id, process_start_id(record.pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => process_start_time(record.pid)
            .is_none_or(|start| start <= record.started_at.saturating_add(START_SLACK)),
    }
}

/// An opaque identity of when `pid` started that is fixed for the process's
/// life, or `None` if unknown: microseconds since the epoch as recorded at
/// fork on macOS, clock ticks since boot on Linux. Unlike [`process_start_time`]
/// on Linux, it does not move when the wall clock steps.
#[cfg(target_os = "macos")]
pub fn process_start_id(pid: u32) -> Option<u64> {
    let info = bsd_info(pid)?;
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// An opaque identity of when `pid` started that is fixed for the process's
/// life, or `None` if unknown: microseconds since the epoch as recorded at
/// fork on macOS, clock ticks since boot on Linux. Unlike [`process_start_time`]
/// on Linux, it does not move when the wall clock steps.
#[cfg(target_os = "linux")]
pub fn process_start_id(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the parenthesized command name start at field 3; the start
    // time in clock ticks since boot is field 22.
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(22 - 3)?.parse().ok()
}

/// An opaque identity of when `pid` started, or `None` if unknown.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn process_start_id(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok().filter(|&pid| pid > 0)?;
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: `proc_bsdinfo` is plain data, so all zeroes is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a live, writable `proc_bsdinfo` of `size` bytes.
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (written == size).then_some(info)
}

/// When `pid` started, in Unix seconds, or `None` if unknown.
#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    bsd_info(pid).map(|info| info.pbi_start_tvsec)
}

/// When `pid` started, in Unix seconds, or `None` if unknown. This adds the
/// boot time, which moves when the wall clock steps.
#[cfg(target_os = "linux")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    let ticks = process_start_id(pid)?;
    // SAFETY: `sysconf` has no preconditions.
    let hertz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hertz = u64::try_from(hertz).ok().filter(|&hertz| hertz > 0)?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(boot + ticks / hertz)
}

/// When `pid` started, in Unix seconds, or `None` if unknown.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn process_start_time(_pid: u32) -> Option<u64> {
    None
}

/// Whether `pid` names a live process. `kill(pid, 0)` probes existence without
/// signaling: 0 means alive; EPERM means alive but owned by another user.
/// Pids 0 and 1 and values outside the platform pid range are never live.
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 1 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Seconds since the Unix epoch, or 0 if the clock precedes it.
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A child forked by a concurrent test briefly inherits every open
    /// descriptor, and with it any `flock`, until it execs. Serialize these
    /// tests so a released lock is observably free.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Scratch(
        PathBuf,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    );
    impl Scratch {
        fn new() -> Self {
            let guard = SERIAL
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "leanck-slots-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path, guard)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One fixed start time, taken after this test process started, so records
    /// of this process stay live.
    static STARTED: std::sync::LazyLock<u64> = std::sync::LazyLock::new(now_unix_secs);

    fn record(pid: u32) -> SlotRecord {
        SlotRecord {
            pid,
            worktree: PathBuf::from(format!("/wt/{pid}")),
            socket: PathBuf::from(format!("/run/leanck-{pid}.sock")),
            started_at: *STARTED,
            start_id: None,
        }
    }

    fn acquired(outcome: SlotOutcome) -> (SlotLock, u64) {
        match outcome {
            SlotOutcome::Acquired { lock, running } => (lock, running),
            other => panic!("expected a slot, got {other:?}"),
        }
    }

    fn attempt(directory: &Path, limit: u64, policy: SlotPolicy) -> SlotOutcome {
        SlotLock::acquire(directory, limit, policy, &record(std::process::id())).unwrap()
    }

    fn take(directory: &Path, limit: u64, policy: SlotPolicy) -> SlotLock {
        acquired(attempt(directory, limit, policy)).0
    }

    fn held(path: &Path) -> bool {
        std::fs::File::open(path).unwrap().try_lock().is_err()
    }

    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// A `sleep 30` child, killed and reaped on drop.
    struct Sleeper(std::process::Child);
    impl Sleeper {
        fn spawn() -> Self {
            Self(
                std::process::Command::new("sleep")
                    .arg("30")
                    .spawn()
                    .unwrap(),
            )
        }
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn two_handles_in_one_process_conflict() {
        let scratch = Scratch::new();
        let path = scratch.0.join("probe.lock");
        let first = std::fs::File::create(&path).unwrap();
        first.try_lock().unwrap();
        assert!(matches!(
            std::fs::File::open(&path).unwrap().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(first);
        assert!(std::fs::File::open(&path).unwrap().try_lock().is_ok());
    }

    #[test]
    fn acquisition_takes_the_lowest_free_slot() {
        let scratch = Scratch::new();
        let (zero, first) = acquired(attempt(&scratch.0, 0, SlotPolicy::Continue));
        let (one, second) = acquired(attempt(&scratch.0, 0, SlotPolicy::Continue));
        assert_eq!((zero.slot(), one.slot()), (0, 1));
        assert_eq!((first, second), (1, 2));
        let two = take(&scratch.0, 0, SlotPolicy::Continue);
        assert_eq!(two.slot(), 2);
        assert_eq!(two.path(), slot_path(&scratch.0, 2));
        drop(zero);
        let (again, running) = acquired(attempt(&scratch.0, 0, SlotPolicy::Continue));
        assert_eq!((again.slot(), running), (0, 3));
        drop((again, one, two));
    }

    #[test]
    fn admission_lock_is_not_a_slot_and_is_released() {
        let scratch = Scratch::new();
        assert_eq!(parse_slot_name(OsStr::new(ADMISSION_LOCK)), None);
        let lock = take(&scratch.0, 0, SlotPolicy::Continue);
        let admission = scratch.0.join(ADMISSION_LOCK);
        assert!(admission.exists());
        assert!(
            !held(&admission),
            "admission is held only during acquisition"
        );
        assert!(matches!(
            attempt(&scratch.0, 1, SlotPolicy::Refuse),
            SlotOutcome::OverLimit { held: 1, limit: 1 }
        ));
        assert!(!held(&admission), "refusal releases admission");
        assert_eq!(scan(&scratch.0).len(), 1);
        drop(lock);
    }

    #[test]
    fn refuse_counts_held_slots_above_a_free_low_slot() {
        let scratch = Scratch::new();
        let mut slots: Vec<_> = (0..3)
            .map(|_| take(&scratch.0, 0, SlotPolicy::Continue))
            .collect();
        drop(slots.remove(0));
        let zero = slot_path(&scratch.0, 0);
        assert!(matches!(
            attempt(&scratch.0, 2, SlotPolicy::Refuse),
            SlotOutcome::OverLimit { held: 2, limit: 2 }
        ));
        assert!(!held(&zero), "a refused acquisition releases slot 0");
        assert_eq!(std::fs::read_to_string(&zero).unwrap(), "");
        assert!(!slot_path(&scratch.0, 3).exists());

        let (lock, running) = acquired(attempt(&scratch.0, 3, SlotPolicy::Refuse));
        assert_eq!((lock.slot(), running), (0, 3));
        drop((lock, slots));
    }

    #[test]
    fn free_existing_slot_files_are_not_counted() {
        let scratch = Scratch::new();
        let zero = take(&scratch.0, 0, SlotPolicy::Continue);
        let five = slot_path(&scratch.0, 5);
        std::fs::write(slot_path(&scratch.0, 1), "").unwrap();
        std::fs::write(&five, "").unwrap();
        let (one, running) = acquired(attempt(&scratch.0, 2, SlotPolicy::Refuse));
        assert_eq!((one.slot(), running), (1, 2));
        assert!(!held(&five), "counting releases a free slot at once");
        assert_eq!(std::fs::read_to_string(&five).unwrap(), "");
        // Slot 2 does not exist and two slots are held: refuse without creating it.
        assert!(matches!(
            attempt(&scratch.0, 2, SlotPolicy::Refuse),
            SlotOutcome::OverLimit { held: 2, limit: 2 }
        ));
        assert!(!slot_path(&scratch.0, 2).exists());
        drop((zero, one));
    }

    #[test]
    fn refuse_stops_at_the_limit_without_touching_the_next_slot() {
        let scratch = Scratch::new();
        let held: Vec<_> = (0..3)
            .map(|_| take(&scratch.0, 3, SlotPolicy::Refuse))
            .collect();
        assert!(matches!(
            attempt(&scratch.0, 3, SlotPolicy::Refuse),
            SlotOutcome::OverLimit { held: 3, limit: 3 }
        ));
        assert!(!slot_path(&scratch.0, 3).exists());
        drop(held);
    }

    #[test]
    fn continue_and_unlimited_scan_past_held_slots() {
        let scratch = Scratch::new();
        let first = take(&scratch.0, 1, SlotPolicy::Refuse);
        let (warned, running) = acquired(attempt(&scratch.0, 1, SlotPolicy::Continue));
        assert_eq!((warned.slot(), running), (1, 2));
        let (unlimited, running) = acquired(attempt(&scratch.0, 0, SlotPolicy::Refuse));
        assert_eq!((unlimited.slot(), running), (2, 3));
        drop((first, warned, unlimited));
    }

    #[test]
    fn acquisition_rewrites_existing_contents_with_the_record() {
        let scratch = Scratch::new();
        let path = slot_path(&scratch.0, 0);
        std::fs::write(
            &path,
            "garbage that is much longer than any JSON record would be ".repeat(8),
        )
        .unwrap();
        let lock = take(&scratch.0, 0, SlotPolicy::Continue);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.ends_with('\n') && text.lines().count() == 1,
            "{text:?}"
        );
        let parsed: SlotRecord = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(parsed, record(std::process::id()));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "pid": std::process::id(),
                "worktree": format!("/wt/{}", std::process::id()),
                "socket": format!("/run/leanck-{}.sock", std::process::id()),
                "started_at": *STARTED,
            })
        );
        drop(lock);
        assert!(path.exists());
        assert!(scan(&scratch.0).is_empty());
    }

    #[test]
    fn scan_and_live_skip_unreadable_records_and_never_lock_or_create() {
        let scratch = Scratch::new();
        let own = take(&scratch.0, 0, SlotPolicy::Continue);
        let dead = record(dead_pid());
        std::fs::write(
            slot_path(&scratch.0, 1),
            format!("{}\n", serde_json::to_string(&dead).unwrap()),
        )
        .unwrap();
        std::fs::write(slot_path(&scratch.0, 2), "{not json").unwrap();
        std::fs::write(slot_path(&scratch.0, 3), "").unwrap();
        std::fs::write(scratch.0.join("leanck-slot-04.lock"), "").unwrap();
        std::fs::write(scratch.0.join("leanck-0123456789abcdef.lock"), "").unwrap();
        let scanned = scan(&scratch.0);
        assert_eq!(
            scanned.iter().map(|e| e.slot).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(scanned[1].record, dead);
        let live = live(&scratch.0);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].slot, 0);
        assert_eq!(live[0].record, record(std::process::id()));
        assert!(held(own.path()));
        drop(own);

        let missing = scratch.0.join("missing");
        assert!(scan(&missing).is_empty());
        assert!(super::live(&missing).is_empty());
        assert!(!missing.exists());
    }

    #[test]
    fn slot_names_round_trip_canonically() {
        let directory = Path::new("/runtime");
        for slot in [0, 7, 4095, u32::MAX] {
            let path = slot_path(directory, slot);
            assert_eq!(parse_slot_name(path.file_name().unwrap()), Some(slot));
        }
        for name in [
            "leanck-slot-.lock",
            "leanck-slot-01.lock",
            "leanck-slot-+1.lock",
            "leanck-slot-1.sock",
            "leanck-slot-4294967296.lock",
            "slot-1.lock",
        ] {
            assert_eq!(parse_slot_name(OsStr::new(name)), None, "{name}");
        }
    }

    #[test]
    fn records_with_reused_pids_are_not_live() {
        let scratch = Scratch::new();
        let now = now_unix_secs();
        let own = SlotRecord {
            started_at: now,
            ..record(std::process::id())
        };
        assert!(record_is_live(&own));

        let sleeper = Sleeper::spawn();
        let fresh = SlotRecord {
            started_at: now_unix_secs(),
            ..record(sleeper.pid())
        };
        assert!(record_is_live(&fresh));
        let reused = SlotRecord {
            started_at: now - 86_400,
            ..fresh.clone()
        };
        assert!(pid_alive(reused.pid));
        assert!(!record_is_live(&reused));
        std::fs::write(
            slot_path(&scratch.0, 0),
            format!("{}\n", serde_json::to_string(&reused).unwrap()),
        )
        .unwrap();
        assert_eq!(scan(&scratch.0).len(), 1);
        assert!(live(&scratch.0).is_empty());
        drop(sleeper);
        assert!(!record_is_live(&fresh));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_known_start_id_decides_liveness_regardless_of_the_wall_clock() {
        let _serial = Scratch::new();
        let sleeper = Sleeper::spawn();
        let id = process_start_id(sleeper.pid()).expect("child start id");
        assert_eq!(process_start_id(sleeper.pid()), Some(id), "stable");
        // A wall clock stepped across the record makes `started_at` disagree
        // with the process start time in either direction; the id still matches.
        for started_at in [0, now_unix_secs() + 86_400] {
            let record = SlotRecord {
                started_at,
                start_id: Some(id),
                ..record(sleeper.pid())
            };
            assert!(record_is_live(&record), "{started_at}");
        }
        let reused = SlotRecord {
            started_at: now_unix_secs(),
            start_id: Some(id ^ 1),
            ..record(sleeper.pid())
        };
        assert!(!record_is_live(&reused));
        let matching = SlotRecord {
            start_id: Some(id),
            ..record(sleeper.pid())
        };
        drop(sleeper);
        assert!(!record_is_live(&matching));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn process_start_time_is_known_for_live_processes_only() {
        let _serial = Scratch::new();
        let now = now_unix_secs();
        let own = process_start_time(std::process::id()).expect("own start time");
        assert!(own <= now && now - own <= 60, "started {own}, now {now}");
        let sleeper = Sleeper::spawn();
        let child = process_start_time(sleeper.pid()).expect("child start time");
        assert!(child >= own && child <= now_unix_secs(), "{child}");
        drop(sleeper);
        let dead = dead_pid();
        assert!(process_start_time(dead).is_none() || pid_alive(dead));
        for pid in [0, u32::MAX] {
            assert_eq!(process_start_time(pid), None, "{pid}");
        }
    }

    #[test]
    fn pid_liveness_rejects_reserved_and_dead_pids() {
        let _serial = Scratch::new();
        assert!(pid_alive(std::process::id()));
        for pid in [0, 1, u32::MAX] {
            assert!(!pid_alive(pid), "{pid}");
        }
        assert!(!pid_alive(dead_pid()));
        assert!(now_unix_secs() > 1_700_000_000);
    }
}
