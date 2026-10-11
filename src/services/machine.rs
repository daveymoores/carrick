//! What this machine can hold: the memory it can hand out now, a process's
//! resident size, and how many of one kind of work fit beside each other.
//!
//! Two callers size work by the same rule with different limits: a sidecar
//! pool (several processes over one service's program, carrick#1996) and the
//! services of a scan analysed side by side (carrick#1066). Each states its
//! limits as a [`Sizing`]; the arithmetic and the log line are one.

/// Memory left free for everything else on the machine (the scanner, the
/// editor and browser of whoever runs the scan), never handed out.
pub const HEADROOM_MB: u64 = 2048;

/// The limits one kind of work is sized under.
#[derive(Debug, Clone, Copy)]
pub struct Sizing {
    /// What is counted, for the log line: `"process(es)"`, `"service(s)"`.
    pub noun: &'static str,
    /// The env var that sets the count instead, named in the log line.
    pub knob: &'static str,
    /// The most there are, whatever the machine holds.
    pub cap: usize,
    /// The fewest the cores allow: one less than the cores, and never fewer
    /// than this.
    pub cores_floor: usize,
    /// How many are already running before any is started, counted in the
    /// total but not against the free memory: a pool's caller's own process.
    pub running: usize,
    /// Whether a count set by [`Self::knob`] is still capped by memory.
    pub knob_capped_by_memory: bool,
}

/// How many, and why, for the scan's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Size {
    /// Never below one.
    pub count: usize,
    pub why: String,
}

/// How many of the work `sizing` describes fit at once, each taking about
/// `per_mb`: as many as the free memory holds after [`HEADROOM_MB`], no more
/// than one per spare core (at least [`Sizing::cores_floor`]) and
/// [`Sizing::cap`] in all. `forced` (the knob's value) sets the count instead.
pub fn size_for(
    sizing: &Sizing,
    available_mb: Option<u64>,
    per_mb: u64,
    cores: usize,
    forced: Option<usize>,
) -> Size {
    let Sizing {
        noun,
        knob,
        cap,
        cores_floor,
        running,
        knob_capped_by_memory,
    } = *sizing;
    let per = per_mb.max(1);
    let Some(available) = available_mb else {
        return match forced {
            Some(count) if !knob_capped_by_memory => Size {
                count,
                why: format!("{count} {noun}: asked for by {knob}"),
            },
            _ => Size {
                count: 1,
                why: format!("one {}: the free memory could not be read", singular(noun)),
            },
        };
    };
    let by_memory = running + (available.saturating_sub(HEADROOM_MB) / per) as usize;
    let by_cores = cores.saturating_sub(1).max(cores_floor);
    let wanted = forced.unwrap_or_else(|| by_cores.min(cap));
    let count = if forced.is_some() && !knob_capped_by_memory {
        wanted
    } else {
        wanted.min(by_memory)
    }
    .max(1);
    let why = format!(
        "{count} {noun}: {wanted} {} and {available} MB free holds {by_memory} at {per} MB each after {HEADROOM_MB} MB kept back",
        if forced.is_some() {
            format!("asked for by {knob}")
        } else {
            format!("allowed by {cores} core(s)")
        },
    );
    Size { count, why }
}

fn singular(noun: &str) -> &str {
    noun.strip_suffix("(es)")
        .or_else(|| noun.strip_suffix("(s)"))
        .unwrap_or(noun)
}

/// The cores this process may use, at least one.
pub fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Memory the machine can hand out now, in MB: what is free plus what the
/// system would reclaim first. `None` when it cannot be read.
pub fn available_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = meminfo
            .lines()
            .find_map(|line| line.strip_prefix("MemAvailable:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        let available = kb / 1024;
        // Inside a container the host's figure is not ours to spend.
        let cgroup = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
            .ok()
            .and_then(|max| max.trim().parse::<u64>().ok())
            .zip(
                std::fs::read_to_string("/sys/fs/cgroup/memory.current")
                    .ok()
                    .and_then(|used| used.trim().parse::<u64>().ok()),
            )
            .map(|(max, used)| max.saturating_sub(used) / (1024 * 1024));
        return Some(cgroup.map_or(available, |left| left.min(available)));
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("vm_stat").output().ok()?;
        return vm_stat_available_mb(&String::from_utf8_lossy(&out.stdout));
    }
    #[allow(unreachable_code)]
    None
}

/// Free, inactive and speculative pages from `vm_stat`'s output, in MB.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn vm_stat_available_mb(text: &str) -> Option<u64> {
    let page: u64 = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |label: &str| -> Option<u64> {
        text.lines()
            .find_map(|line| line.strip_prefix(label))?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    let total = pages("Pages free:")? + pages("Pages inactive:")? + pages("Pages speculative:")?;
    Some(total * page / (1024 * 1024))
}

/// The resident size of process `pid` in bytes, from `proc_pidinfo`.
///
/// Asked of the kernel, not of `ps`: `ps` is setuid on macOS, so a sandbox
/// that refuses setuid binaries refuses it, and a slim container may not
/// have it.
#[cfg(target_os = "macos")]
pub fn resident_bytes(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_taskinfo>()).ok()?;
    // SAFETY: a zeroed `proc_taskinfo` is a valid value of that plain C struct.
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    // SAFETY: `proc_pidinfo` writes at most `size` bytes, the size of `info`.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut libc::proc_taskinfo).cast(),
            size,
        )
    };
    (written == size).then_some(info.pti_resident_size)
}

/// The resident size of process `pid` in bytes, from `/proc/<pid>/status`.
#[cfg(target_os = "linux")]
pub fn resident_bytes(pid: u32) -> Option<u64> {
    proc_status_kb(pid, "VmRSS:").map(|kb| kb * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn resident_bytes(_pid: u32) -> Option<u64> {
    None
}

/// The most memory process `pid` has held so far, in bytes: its lifetime
/// peak physical footprint, from `proc_pid_rusage`. Read while it runs, so
/// the process is not reaped for it.
#[cfg(target_os = "macos")]
pub fn peak_resident_bytes(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: a zeroed `rusage_info_v4` is a valid value of that plain C struct.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: with `RUSAGE_INFO_V4` the kernel writes one `rusage_info_v4`.
    let status = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            (&mut info as *mut libc::rusage_info_v4).cast(),
        )
    };
    (status == 0).then_some(info.ri_lifetime_max_phys_footprint)
}

/// The most memory process `pid` has held so far, in bytes: its resident
/// high-water mark, from `/proc/<pid>/status`.
#[cfg(target_os = "linux")]
pub fn peak_resident_bytes(pid: u32) -> Option<u64> {
    proc_status_kb(pid, "VmHWM:").map(|kb| kb * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn peak_resident_bytes(_pid: u32) -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn proc_status_kb(pid: u32, field: &str) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live child's resident size and its peak are read from the kernel: a
    /// node process that holds 64 MB it has written to reads as at least that
    /// much, now and at its peak. No other process is started for the read,
    /// so it works where `ps` cannot run.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn a_live_child_s_resident_size_and_peak_are_read_from_the_kernel() {
        use std::io::BufRead;

        let mut child = std::process::Command::new("node")
            .args([
                "-e",
                "globalThis.held = Buffer.alloc(64 * 1024 * 1024, 1); \
                 console.log('ready'); setTimeout(() => {}, 60000);",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("node starts");
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "ready");

        let bytes = resident_bytes(child.id());
        let peak = peak_resident_bytes(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let bytes = bytes.expect("the child's size reads");
        assert!(bytes >= 64 * 1024 * 1024, "{bytes} bytes");
        let peak = peak.expect("the child's peak reads");
        assert!(peak >= 64 * 1024 * 1024, "{peak} bytes at its peak");
        assert_eq!(resident_bytes(u32::MAX), None, "no such process");
        assert_eq!(peak_resident_bytes(u32::MAX), None, "no such process");
    }

    #[test]
    fn vm_stat_reads_free_inactive_and_speculative_pages() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                    Pages free:                               65536.\n\
                    Pages active:                            900000.\n\
                    Pages inactive:                          131072.\n\
                    Pages speculative:                         1024.\n";
        // (65536 + 131072 + 1024) * 16 KB = 3088 MB.
        assert_eq!(vm_stat_available_mb(text), Some(3088));
        assert_eq!(vm_stat_available_mb("no header"), None);
    }
}
