use std::{
    env,
    ffi::OsString,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            net::UnixStream,
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::Path,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

fn group(pid: libc::pid_t) -> io::Result<libc::pid_t> {
    // SAFETY: getpgid takes only a PID and does not access Rust memory.
    let group = unsafe { libc::getpgid(pid) };
    if group < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(group)
    }
}

fn parent() -> libc::pid_t {
    // SAFETY: no arguments or memory accesses.
    unsafe { libc::getppid() }
}

fn kill_group(pid: libc::pid_t) -> io::Result<()> {
    // Self-group termination is used only by the isolated inner supervisor.
    if pid <= 1 || (pid != std::process::id().cast_signed() && pid == group(0)?) {
        return Err(io::Error::other(
            "refusing unsafe process-group termination",
        ));
    }
    // SAFETY: pid is the validated owned group; SIGKILL does not target the caller's group.
    if unsafe { libc::kill(-pid, libc::SIGKILL) } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

struct GroupChild {
    child: Child,
    pid: libc::pid_t,
}
impl Drop for GroupChild {
    fn drop(&mut self) {
        // Command::process_group established ownership before exec. Keep the leader
        // unreaped until here; it remains alive after the server's ordinary exit.
        let owned = match group(self.pid) {
            Ok(id) => id == self.pid,
            Err(error) => error.raw_os_error() == Some(libc::ESRCH),
        };
        let result = if owned {
            kill_group(self.pid)
        } else {
            self.child.kill()
        };
        if let Err(error) = result {
            eprintln!("rust-guard: cleanup failed: {error}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Err(error) => {
                    eprintln!("rust-guard: could not reap supervisor: {error}");
                    break;
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    eprintln!("rust-guard: supervisor did not exit during cleanup");
                    break;
                }
            }
        }
    }
}

pub(super) fn run(executable: &Path, arguments: &[OsString], limit_mib: usize) -> io::Result<u32> {
    let (mut control, child_control) = UnixStream::pair()?;
    let descriptor = child_control.as_raw_fd();
    let mut command = Command::new(env::current_exe()?);
    command
        .arg("--guard-unix-child")
        .arg(std::process::id().to_string())
        .arg(descriptor.to_string())
        .arg(executable)
        .args(arguments)
        .process_group(0);
    // SAFETY: only an async-signal-safe syscall in the forked child; no allocation,
    // locking, or logging. Parent descriptors retain their close-on-exec flags.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(descriptor, libc::F_SETFD, 0) == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let child = command.spawn()?;
    let pid = child.id().cast_signed(); // OS PIDs are positive pid_t values.
    let _owned = GroupChild { child, pid };
    drop(child_control);
    control.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut ready = [0];
    control.read_exact(&mut ready)?;
    if ready != [1] || pid <= 1 || group(pid)? != pid {
        return Err(io::Error::other(
            "child did not establish its own process group",
        ));
    }
    eprintln!("rust-guard: soft RSS watchdog; polling may overshoot the memory budget.");
    control.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut status = [0; 4];
    let mut received = 0;
    loop {
        match control.read(&mut status[received..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "internal supervisor closed its control channel",
                ));
            }
            Ok(count) => {
                received += count;
                if received == status.len() {
                    return Ok(u32::from_le_bytes(status));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        // EOF detects supervisor death. Reap only after group cleanup so its PID
        // remains reserved even when the leader exits unexpectedly.
        if resident_bytes(pid)? >= (limit_mib * 1024 * 1024) as u64 {
            eprintln!("rust-guard: LSP stopped at its {limit_mib} MiB RSS budget.");
            return Ok(super::BUDGET);
        }
    }
}

fn check_parent(expected: libc::pid_t, own_group: libc::pid_t) -> io::Result<()> {
    if parent() != expected {
        kill_group(own_group)?;
    }
    Ok(())
}

pub(super) fn run_child(arguments: &[OsString]) -> io::Result<u32> {
    let number = |index: usize| {
        arguments
            .get(index)
            .and_then(|value| value.to_str())
            .and_then(|value| value.parse::<i32>().ok())
    };
    let (Some(expected), Some(descriptor)) = (number(0), number(1)) else {
        return Err(io::Error::other("invalid internal launcher invocation"));
    };
    let own_group = std::process::id().cast_signed();
    if arguments.len() < 3
        || expected <= 1
        || descriptor < 3
        || parent() != expected
        || own_group <= 1
        || group(0)? != own_group
    {
        return Err(io::Error::other("invalid internal launcher invocation"));
    }
    // SAFETY: the inherited socket is exclusively transferred from the outer process;
    // this internal mode validates its descriptor before wrapping it with one owner.
    if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut control = unsafe { UnixStream::from_raw_fd(descriptor) };
    let result = (|| {
        check_parent(expected, own_group)?;
        control.write_all(&[1])?;
        let code = match Command::new(&arguments[2]).args(&arguments[3..]).spawn() {
            Ok(mut server) => loop {
                check_parent(expected, own_group)?;
                if let Some(status) = server.try_wait()? {
                    break status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
                        .cast_unsigned();
                }
                thread::sleep(Duration::from_millis(50));
            },
            Err(error) => {
                eprintln!("rust-guard: could not launch LSP: {error}");
                super::FAILURE
            }
        };
        control.write_all(&code.to_le_bytes())?;
        // Keep group ownership anchored until outer cleanup, including rapid server exits.
        loop {
            check_parent(expected, own_group)?;
            thread::sleep(Duration::from_millis(50));
        }
    })();
    if let Err(error) = &result {
        eprintln!("rust-guard: internal supervisor failed: {error}");
    }
    kill_group(own_group)?;
    result
}

#[cfg(target_os = "linux")]
fn resident_bytes(owned_group: libc::pid_t) -> io::Result<u64> {
    // SAFETY: sysconf reads the host's page-size constant.
    let page_size = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
        .ok()
        .filter(|size| *size > 0)
        .ok_or_else(|| io::Error::other("invalid page size"))?;
    let mut total = 0_u64;
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        match group(pid) {
            Ok(id) if id == owned_group => {}
            Ok(_) => continue,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(error) => return Err(error),
        }
        let statm = match std::fs::read_to_string(entry.path().join("statm")) {
            Ok(statm) => statm,
            Err(_) if group(pid).is_err_and(|error| error.raw_os_error() == Some(libc::ESRCH)) => {
                continue;
            }
            Err(error) => return Err(error),
        };
        let pages = statm
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| io::Error::other("invalid /proc resident-memory sample"))?;
        total = pages
            .checked_mul(page_size)
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or_else(|| io::Error::other("resident-memory accounting overflow"))?;
    }
    Ok(total)
}

#[cfg(target_os = "macos")]
fn resident_bytes(owned_group: libc::pid_t) -> io::Result<u64> {
    use std::{
        mem::{size_of, zeroed},
        ptr::null_mut,
    };
    // SAFETY: null requests a size estimate. The API returns a count of PIDs, not bytes.
    let estimate = unsafe { libc::proc_listpgrppids(owned_group, null_mut(), 0) };
    if estimate <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0_i32; usize::try_from(estimate).map_err(io::Error::other)? + 32];
    let count = loop {
        let bytes =
            i32::try_from(pids.len() * size_of::<libc::pid_t>()).map_err(io::Error::other)?;
        // SAFETY: buffer is correctly aligned and bytes describes its writable extent.
        let count =
            unsafe { libc::proc_listpgrppids(owned_group, pids.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return Err(io::Error::last_os_error());
        }
        let count = usize::try_from(count).map_err(io::Error::other)?;
        if count < pids.len() {
            break count;
        }
        pids.resize(
            pids.len()
                .checked_mul(2)
                .ok_or_else(|| io::Error::other("PID buffer overflow"))?,
            0,
        );
    };
    let mut total = 0_u64;
    for &pid in &pids[..count] {
        // SAFETY: proc_taskinfo contains numeric fields; proc_pidinfo receives its exact size.
        let mut info: libc::proc_taskinfo = unsafe { zeroed() };
        let size = i32::try_from(size_of::<libc::proc_taskinfo>()).map_err(io::Error::other)?;
        let result = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast(), size)
        };
        if result != size {
            let error = io::Error::last_os_error();
            if group(pid).is_err_and(|error| error.raw_os_error() == Some(libc::ESRCH)) {
                continue;
            }
            return Err(io::Error::other(format!(
                "incomplete proc_pidinfo sample ({result}/{size}): {error}"
            )));
        }
        total = total
            .checked_add(info.pti_resident_size)
            .ok_or_else(|| io::Error::other("resident-memory accounting overflow"))?;
    }
    Ok(total)
}
