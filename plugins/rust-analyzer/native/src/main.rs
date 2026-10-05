use std::{env, ffi::OsString, io, path::PathBuf};

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(windows, target_os = "linux", target_os = "macos")
)))]
compile_error!("Supported hosts: Windows, Linux and macOS on x64 or ARM64.");

const FAILURE: u32 = 125;
const BUDGET: u32 = 137;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("rust-guard: refusing unguarded launch: {error}");
            FAILURE
        }
    };
    // Preserve all Windows exit-code bits; Unix uses the low eight bits.
    std::process::exit(i32::from_ne_bytes(code.to_ne_bytes()));
}

fn run() -> io::Result<u32> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    #[cfg(unix)]
    if arguments
        .first()
        .is_some_and(|arg| arg == "--guard-unix-child")
    {
        return unix::run_child(&arguments[1..]);
    }
    let budget = match env::var("RUST_ANALYZER_MEMORY_LIMIT_MIB") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => "6144".into(),
        Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    };
    let limit = parse_budget(&budget)?;
    let executable =
        resolve(env::var_os("RUST_ANALYZER_EXECUTABLE").unwrap_or_else(|| "rust-analyzer".into()))?;
    #[cfg(windows)]
    return windows::run(&executable, &arguments, limit);
    #[cfg(unix)]
    return unix::run(&executable, &arguments, limit);
}

fn parse_budget(value: &str) -> io::Result<usize> {
    if !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(limit @ 32..=1_048_576) = value.parse()
    {
        return Ok(limit);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "memory budget must be whole MiB between 32 and 1048576",
    ))
}

fn resolve(value: OsString) -> io::Result<PathBuf> {
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return validate(path);
    }
    if value.is_empty()
        || value
            .as_encoded_bytes()
            .iter()
            .any(|byte| b"/\\:".contains(byte))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "relative paths are not supported; use an absolute executable or a PATH name",
        ));
    }
    #[cfg(windows)]
    let value = {
        let mut name = value;
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            name.push(".exe");
        }
        name
    };
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        if !directory.is_absolute() {
            continue;
        }
        let candidate = directory.join(&value);
        if candidate.is_file() {
            return validate(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("server not found on PATH: {}", value.to_string_lossy()),
    ))
}

fn validate(path: PathBuf) -> io::Result<PathBuf> {
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("server not found: {}", path.display()),
        ));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows server must be a native .exe",
        ));
    }
    Ok(path)
}
