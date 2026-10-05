use std::{
    ffi::OsStr,
    ffi::OsString,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NO_WINDOW, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
            InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

fn require(success: i32) -> io::Result<()> {
    if success == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in executable or argument",
        ));
    }
    result.push(0);
    Ok(result)
}

fn quote(value: &OsStr, result: &mut Vec<u16>) -> io::Result<()> {
    let value = wide(value)?;
    result.push(34);
    let mut slashes = 0;
    for &unit in &value[..value.len() - 1] {
        if unit == 92 {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            92,
            if unit == 34 { slashes * 2 + 1 } else { slashes },
        ));
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(92, slashes * 2));
    result.push(34);
    Ok(())
}

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: null requests the required size; storage is aligned and then initialized.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size);
        }
        if size == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0; size.div_ceil(size_of::<usize>())];
        unsafe {
            require(InitializeProcThreadAttributeList(
                storage.as_mut_ptr().cast(),
                2,
                0,
                &mut size,
            ))?;
        }
        Ok(Self { storage })
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: new initialized this allocation and it has not moved or been freed.
        unsafe {
            DeleteProcThreadAttributeList(self.ptr());
        }
    }
}

pub(super) fn run(executable: &Path, arguments: &[OsString], limit_mib: usize) -> io::Result<u32> {
    let executable_wide = wide(executable.as_os_str())?;
    let mut command = Vec::new();
    quote(executable.as_os_str(), &mut command)?;
    for argument in arguments {
        command.push(32);
        quote(argument, &mut command)?;
    }
    command.push(0);
    let limit_bytes = limit_mib * 1024 * 1024;
    // SAFETY: the structures contain only numeric fields/pointers; buffers and handles
    // remain alive throughout CreateProcessW. Each successful handle gets one owner.
    unsafe {
        let raw_job = CreateJobObjectW(null(), null());
        if raw_job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(raw_job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        limits.JobMemoryLimit = limit_bytes;
        let limit_size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(io::Error::other)?;
        require(SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            limit_size,
        ))?;

        let mut owned_streams = Vec::new();
        for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let mut handle = null_mut();
            require(DuplicateHandle(
                GetCurrentProcess(),
                GetStdHandle(kind),
                GetCurrentProcess(),
                &mut handle,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            ))?;
            owned_streams.push(OwnedHandle::from_raw_handle(handle));
        }
        let streams: Vec<HANDLE> = owned_streams
            .iter()
            .map(AsRawHandle::as_raw_handle)
            .collect();
        let jobs = [job.as_raw_handle()];
        let mut attributes = Attributes::new()?;
        require(UpdateProcThreadAttribute(
            attributes.ptr(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            streams.as_ptr().cast(),
            size_of::<HANDLE>() * streams.len(),
            null_mut(),
            null(),
        ))?;
        // Atomically assign the job before the process executes; no suspended orphan window.
        require(UpdateProcThreadAttribute(
            attributes.ptr(),
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            jobs.as_ptr().cast(),
            size_of::<HANDLE>(),
            null_mut(),
            null(),
        ))?;
        let mut startup: STARTUPINFOEXW = zeroed();
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).map_err(io::Error::other)?;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = streams[0];
        startup.StartupInfo.hStdOutput = streams[1];
        startup.StartupInfo.hStdError = streams[2];
        startup.lpAttributeList = attributes.ptr();
        let mut process: PROCESS_INFORMATION = zeroed();
        require(CreateProcessW(
            executable_wide.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &startup.StartupInfo,
            &mut process,
        ))?;
        let process_handle = OwnedHandle::from_raw_handle(process.hProcess);
        let _thread_handle = OwnedHandle::from_raw_handle(process.hThread);
        loop {
            match WaitForSingleObject(process_handle.as_raw_handle(), 100) {
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    require(GetExitCodeProcess(
                        process_handle.as_raw_handle(),
                        &mut code,
                    ))?;
                    return Ok(code);
                }
                WAIT_TIMEOUT => {}
                _ => return Err(io::Error::last_os_error()),
            }
            let mut usage: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            require(QueryInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw mut usage).cast(),
                limit_size,
                null_mut(),
            ))?;
            if usage.PeakJobMemoryUsed >= limit_bytes * 95 / 100 {
                require(TerminateJobObject(job.as_raw_handle(), super::BUDGET))?;
                eprintln!(
                    "rust-guard: LSP stopped near its {limit_mib} MiB memory ceiling; Claude is outside this job."
                );
                return Ok(super::BUDGET);
            }
        }
        // Closing the uniquely owned job also cleans up descendants on normal exit/errors.
    }
}
