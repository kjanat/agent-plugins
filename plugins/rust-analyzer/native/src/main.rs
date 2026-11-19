use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::Command,
};

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
const PROXY: &str = "rust-analyzer";

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
    let name = executable_name(value);
    if is_proxy(&name)
        && let Some(rustup) = search(&executable_name("rustup".into()))
    {
        return toolchain_server(&rustup);
    }
    search(&name).map_or_else(
        || {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("server not found on PATH: {}", name.to_string_lossy()),
            ))
        },
        validate,
    )
}

fn executable_name(value: OsString) -> OsString {
    #[cfg(windows)]
    if !Path::new(&value)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        let mut name = value;
        name.push(".exe");
        return name;
    }
    value
}

fn is_proxy(name: &OsStr) -> bool {
    #[cfg(windows)]
    return name.eq_ignore_ascii_case("rust-analyzer.exe");
    #[cfg(not(windows))]
    return name == PROXY;
}

fn search(name: &OsStr) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn toolchain_server(rustup: &Path) -> io::Result<PathBuf> {
    let mut command = Command::new(rustup);
    command.args(["which", PROXY]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        &mut command,
        windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
    );
    let output = command.output().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not run {} which {PROXY}: {error}", rustup.display()),
        )
    })?;
    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{}; run `rustup component add {PROXY}` in this workspace",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let path =
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_end_matches(['\r', '\n']));
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "rustup which {PROXY} returned a relative path: {}",
                path.display()
            ),
        ));
    }
    validate(path)
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
