use std::{
    env,
    io::{self, Write},
    process::{self, Child, Command},
    thread,
    time::Duration,
};

fn child(mode: &str) -> io::Result<Child> {
    let mut command = Command::new(env::current_exe()?);
    command.arg(mode);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command.spawn()
}

#[cfg(windows)]
fn deny_allocation() -> i32 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(
            address: *mut std::ffi::c_void,
            size: usize,
            allocation: u32,
            protection: u32,
        ) -> *mut std::ffi::c_void;
    }
    // SAFETY: request a fresh committed read/write allocation, without dereferencing it.
    let denied =
        unsafe { VirtualAlloc(std::ptr::null_mut(), 512 * 1024 * 1024, 0x3000, 4) }.is_null();
    println!(
        "{}",
        if denied {
            "allocation-denied"
        } else {
            "allocation-granted"
        }
    );
    if denied { 42 } else { 43 }
}

fn rustup() -> i32 {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let variable = |name: &str| env::var(format!("GUARD_TEST_RUSTUP_{name}")).ok();
    let installed = variable("STATE").and_then(|state| std::fs::read_to_string(state).ok());
    let found = match arguments.as_slice() {
        ["which", "rust-analyzer"] => variable("WHICH"),
        ["which", "--toolchain", toolchain, "rust-analyzer"] => installed
            .filter(|installed| installed == toolchain)
            .and(variable("FALLBACK")),
        ["default"] => variable("DEFAULT").map(|toolchain| format!("{toolchain} (default)")),
        [
            "component",
            "add",
            "rust-analyzer",
            "--toolchain",
            toolchain,
        ] => variable("FALLBACK")
            .zip(variable("STATE"))
            .and_then(|(_, state)| std::fs::write(state, toolchain).ok())
            .map(|()| String::new()),
        _ => None,
    };
    match found {
        Some(output) => {
            println!("{output}");
            0
        }
        None => {
            eprintln!("error: fixture rustup refused {}", arguments.join(" "));
            1
        }
    }
}

fn run() -> io::Result<i32> {
    match env::args().nth(1).as_deref() {
        Some("echo") => {
            io::stderr().write_all(b"stderr-only")?;
            io::copy(&mut io::stdin().lock(), &mut io::stdout().lock())?;
            Ok(7)
        }
        Some("args") => {
            for argument in env::args().skip(2) {
                for byte in argument.as_bytes() {
                    print!("{byte:02x}");
                }
                println!();
            }
            Ok(0)
        }
        #[cfg(windows)]
        Some("deny") => Ok(deny_allocation()),
        Some(mode @ ("tree" | "hold")) => {
            let mut spawned = child(if mode == "tree" { "allocate" } else { "stay" })?;
            println!("{} {}", process::id(), spawned.id());
            if mode == "tree" {
                // Let the allocation workload finish even on slower CI runners.
                spawned.wait()?;
            } else {
                thread::sleep(Duration::from_secs(4));
            }
            Ok(0)
        }
        Some("allocate") => {
            let mut blocks = Vec::new();
            let mut random = 0x4d59_5df4_d0f3_3173_u64;
            for _ in 0..160 {
                // Repeated bytes are highly compressible on macOS. Fill every page
                // with deterministic noise to resist memory compression.
                let mut block = vec![0_u64; 2 * 1024 * 1024 / size_of::<u64>()];
                for word in &mut block {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    *word = random;
                }
                blocks.push(block);
                // Keep earlier pages active while the watchdog samples the group.
                // A 4 KiB stride also covers larger native page sizes.
                for block in &mut blocks {
                    for word in block.iter_mut().step_by(4096 / size_of::<u64>()) {
                        *word = std::hint::black_box(*word).wrapping_add(1);
                    }
                }
                thread::sleep(Duration::from_millis(15));
            }
            std::hint::black_box(&blocks);
            thread::sleep(Duration::from_millis(500));
            Ok(0)
        }
        Some("orphan") => {
            println!("{}", child("stay")?.id());
            Ok(0)
        }
        Some("stay") => {
            thread::sleep(Duration::from_secs(4));
            Ok(0)
        }
        Some("which" | "default" | "component") => Ok(rustup()),
        _ => Ok(99),
    }
}

fn main() -> io::Result<()> {
    process::exit(run()?);
}
