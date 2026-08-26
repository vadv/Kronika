//! Parse and read per-process `/proc/PID/*` files.

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;

use crate::ProcFs;

/// Parse error for process procfs content.
pub use crate::proc::stat::ParseError;

mod model;
mod parse;
mod sections;

pub use model::{
    ProcIo, ProcStat, ProcStatus, ProcessCgroupRow, ProcessError, ProcessFacts, ProcessHotRow,
    ProcessRead, ProcessStatusRow,
};
pub use parse::{parse_cgroup_path, parse_io, parse_stat, parse_status};
pub use sections::{to_hot_section, to_status_section};

use parse::{
    i8_from_i64, i16_from_i64, i32_from_i64, normalize_cmdline, parse_btime,
    process_starttime_usec, rss_kb, u8_from_i64, u32_from_i64,
};

/// Read process conversion facts from the same procfs root as process rows.
///
/// # Errors
/// Returns an [`io::Error`] when `/proc/stat` cannot be read or `btime` is
/// absent/malformed.
pub fn process_facts(fs: &ProcFs) -> io::Result<ProcessFacts> {
    let stat = fs.read("stat")?;
    let btime_usec =
        parse_btime(&stat).ok_or_else(|| io::Error::other("stat: no parsable btime line"))?;
    Ok(ProcessFacts {
        btime_usec,
        clock_ticks_per_sec: i64::try_from(rustix::param::clock_ticks_per_second())
            .map_err(io::Error::other)?,
        page_size_bytes: i64::try_from(rustix::param::page_size()).map_err(io::Error::other)?,
    })
}

/// Read one process from `/proc/PID`.
///
/// # Errors
/// Returns [`ProcessError`] when a required file disappears, cannot be read, or
/// cannot be parsed. Optional `io`, `schedstat`, `cmdline`, `comm`, and cgroup
/// read failures are recorded in the returned row instead.
pub fn read_process(
    fs: &ProcFs,
    pid: i32,
    facts: ProcessFacts,
    ts: i64,
) -> Result<ProcessRead, ProcessError> {
    let mut reader = ProcessReader::new(fs);
    let cgroup_path = reader.cgroup_membership(pid).and_then(parse_cgroup_path);
    reader.read(pid, facts, ts, cgroup_path)
}

/// Read one process while reusing an optional, already-read cgroup membership.
///
/// # Errors
/// Returns [`ProcessError`] under the same conditions as [`read_process`].
pub fn read_process_with_cgroup(
    fs: &ProcFs,
    pid: i32,
    facts: ProcessFacts,
    ts: i64,
    cgroup_membership: Option<&str>,
) -> Result<ProcessRead, ProcessError> {
    let mut reader = ProcessReader::new(fs);
    reader.read(
        pid,
        facts,
        ts,
        cgroup_membership.and_then(parse_cgroup_path),
    )
}

/// Reusable scratch storage for sequential `/proc/PID/*` reads.
#[derive(Debug)]
pub struct ProcessReader<'a> {
    fs: &'a ProcFs,
    rel: String,
    path: PathBuf,
    content: String,
    own_uid: u32,
    own_gid: u32,
}

impl<'a> ProcessReader<'a> {
    /// Create a reader for one sequential process scan.
    #[must_use]
    pub fn new(fs: &'a ProcFs) -> Self {
        Self {
            fs,
            rel: String::with_capacity(32),
            path: PathBuf::new(),
            content: String::with_capacity(2 * 1024),
            own_uid: rustix::process::getuid().as_raw(),
            own_gid: rustix::process::getgid().as_raw(),
        }
    }

    /// Read one process cgroup membership, returning `None` on a transient read failure.
    pub fn cgroup_membership(&mut self, pid: i32) -> Option<&str> {
        self.read_raw(pid, "cgroup").ok()
    }

    /// Read one process while reusing this scan's path and content buffers.
    ///
    /// # Errors
    /// Returns [`ProcessError`] under the same conditions as [`read_process`].
    pub fn read(
        &mut self,
        pid: i32,
        facts: ProcessFacts,
        ts: i64,
        cgroup_path: Option<String>,
    ) -> Result<ProcessRead, ProcessError> {
        let stat =
            parse_stat(self.read_required(pid, "stat")?).map_err(|source| ProcessError::Parse {
                path: format!("{pid}/stat"),
                source,
            })?;

        let status = parse_status(self.read_required(pid, "status")?).map_err(|source| {
            ProcessError::Parse {
                path: format!("{pid}/status"),
                source,
            }
        })?;

        let io = self.read_io(pid, status.uid, status.gid);
        let rundelay_ns = self.read_schedstat(pid).unwrap_or(0);
        let cmdline = self.read_cmdline(pid);
        let comm = self.read_comm(pid).unwrap_or_else(|| stat.comm.clone());
        let starttime = process_starttime_usec(facts, stat.starttime_ticks);
        let cgroup = cgroup_path.map(|cgroup_path| ProcessCgroupRow {
            ts,
            pid,
            starttime,
            cgroup_path,
        });

        let hot = ProcessHotRow {
            ts,
            pid: stat.pid,
            starttime,
            ppid: stat.ppid,
            uid: status.uid,
            euid: status.euid,
            gid: status.gid,
            egid: status.egid,
            state: stat.state,
            num_threads: u32_from_i64(stat.num_threads),
            tty: u16::try_from(stat.tty_nr).unwrap_or(0),
            comm,
            cmdline,
            utime: stat.utime,
            stime: stat.stime,
            nice: i8_from_i64(stat.nice),
            prio: i16_from_i64(stat.priority),
            rtprio: i16_from_i64(stat.rt_priority),
            policy: u8_from_i64(stat.policy),
            curcpu: i32_from_i64(stat.processor),
            rundelay_ns,
            blkdelay_ticks: stat.delayacct_blkio_ticks,
            nvcsw: status.voluntary_ctxt_switches,
            nivcsw: status.nonvoluntary_ctxt_switches,
            minflt: stat.minflt,
            majflt: stat.majflt,
            vmem_kb: stat.vsize_bytes / 1024,
            rmem_kb: rss_kb(stat.rss_pages, facts.page_size_bytes),
            vswap_kb: status.vm_swap,
            io,
            exit_signal: i32_from_i64(stat.exit_signal),
        };
        let status = ProcessStatusRow {
            ts,
            pid: stat.pid,
            starttime,
            status,
        };
        Ok(ProcessRead {
            hot,
            status,
            cgroup,
        })
    }

    fn read_required(&mut self, pid: i32, file: &str) -> Result<&str, ProcessError> {
        match self.read_raw(pid, file) {
            Ok(content) => Ok(content),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Err(ProcessError::Gone(pid)),
            Err(source) => Err(ProcessError::Read {
                path: format!("{pid}/{file}"),
                source,
            }),
        }
    }

    fn read_raw(&mut self, pid: i32, file: &str) -> io::Result<&str> {
        self.rel.clear();
        write!(&mut self.rel, "{pid}/{file}")
            .map_err(|_| io::Error::other("failed to construct procfs-relative path"))?;
        self.fs
            .read_raw_into(&self.rel, &mut self.path, &mut self.content)?;
        Ok(&self.content)
    }

    fn read_io(&mut self, pid: i32, uid: u32, gid: u32) -> Option<ProcIo> {
        if needs_fs_creds((uid, gid), (self.own_uid, self.own_gid)) {
            return self.read_io_with_fs_creds(pid, uid, gid);
        }
        self.read_raw(pid, "io").ok().map(parse_io)
    }

    #[cfg(target_os = "linux")]
    fn read_io_with_fs_creds(&mut self, pid: i32, uid: u32, gid: u32) -> Option<ProcIo> {
        let _guard = FsCredGuard::switch(uid, gid);
        self.read_raw(pid, "io").ok().map(parse_io)
    }

    // Non-Linux builds cannot switch filesystem credentials. Keep the method
    // shape used by shared `read_io`.
    #[cfg(not(target_os = "linux"))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "matches the Linux method called by platform-neutral read_io"
    )]
    const fn read_io_with_fs_creds(&mut self, _pid: i32, _uid: u32, _gid: u32) -> Option<ProcIo> {
        None
    }

    fn read_schedstat(&mut self, pid: i32) -> Option<i64> {
        let content = self.read_raw(pid, "schedstat").ok()?;
        let mut fields = content.split_whitespace();
        let _run_time_ns = fields.next()?;
        fields.next()?.parse().ok()
    }

    fn read_cmdline(&mut self, pid: i32) -> Option<String> {
        let content = self.read_raw(pid, "cmdline").ok()?;
        normalize_cmdline(content)
    }

    fn read_comm(&mut self, pid: i32) -> Option<String> {
        let comm = self.read_raw(pid, "comm").ok()?.trim();
        (!comm.is_empty()).then(|| comm.to_owned())
    }
}

/// Whether reading a pid's `io` file needs a filesystem-credential switch
/// first. A plain read only ever succeeds when our own filesystem
/// credentials already match the target process, so a same-identity pid can
/// skip straight to the read instead of paying for a read that is bound to
/// fail with `EACCES`.
const fn needs_fs_creds(target: (u32, u32), own: (u32, u32)) -> bool {
    target.0 != own.0 || target.1 != own.1
}

#[cfg(target_os = "linux")]
struct FsCredGuard {
    uid: nix::unistd::Uid,
    gid: nix::unistd::Gid,
}

#[cfg(target_os = "linux")]
impl FsCredGuard {
    fn switch(uid: u32, gid: u32) -> Self {
        let saved_group = nix::unistd::setfsgid(nix::unistd::Gid::from_raw(gid));
        Self {
            uid: nix::unistd::setfsuid(nix::unistd::Uid::from_raw(uid)),
            gid: saved_group,
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for FsCredGuard {
    fn drop(&mut self) {
        nix::unistd::setfsuid(self.uid);
        nix::unistd::setfsgid(self.gid);
    }
}

#[cfg(test)]
mod tests;
