use std::path::PathBuf;
use std::process::{Command, Stdio};

fn resource_compiler_works(candidate: &std::ffi::OsStr) -> bool {
    Command::new(candidate)
        .arg("/nologo")
        .arg("/?")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn find_resource_compiler() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC") {
        if resource_compiler_works(&rc) {
            return Some(PathBuf::from(rc));
        }
    }
    for name in ["rc.exe", "llvm-rc.exe"] {
        if resource_compiler_works(name.as_ref()) {
            return Some(PathBuf::from(name));
        }
    }
    /* GitHub windows-latest 有 VS Build Tools，但 rc.exe 不一定在 PATH
    （普通 cargo build 由 rustc 自己定位 link.exe，build.rs 不会继承这套
    发现逻辑）。经 vswhere 找 Hostx64 的 rc.exe，避免干净 CI 上测试 manifest
    静默缺失、进程在 main 之前失败。 */
    let vswhere = std::env::var_os("ProgramFiles(x86)")
        .map(|dir| PathBuf::from(dir).join("Microsoft Visual Studio/Installer/vswhere.exe"))?;
    if !vswhere.is_file() {
        return None;
    }
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-find",
            r"VC\Tools\MSVC\**\bin\Hostx64\x64\rc.exe",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
}

fn main() {
    tauri_build::build();
    /* Tauri mock-runtime 集成测试：测试 exe 默认没有应用 manifest，Windows 会加载
    comctl32 v5；Tauri 依赖的 TaskDialogIndirect 只在 v6 中导出，导致进程在
    main 之前报 STATUS_ENTRYPOINT_NOT_FOUND。把 comctl32 v6 依赖编成 .res 资源
    链入测试目标；生产 exe 的 manifest 仍由 tauri-build 生成。 */
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("tests")
            .join("test-manifest.xml");
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let rc_path = out_dir.join("test-manifest.rc");
        let res_path = out_dir.join("test-manifest.res");
        std::fs::write(&rc_path, format!("1 24 \"{}\"\n", manifest.display())).unwrap();
        match find_resource_compiler() {
            Some(rc) => match Command::new(rc)
                .arg("/nologo")
                .arg("/fo")
                .arg(&res_path)
                .arg(&rc_path)
                .status()
            {
                Ok(status) if status.success() => {
                    println!("cargo:rustc-link-arg-tests={}", res_path.display());
                }
                Ok(status) => {
                    println!("cargo:warning=test manifest resource compiler failed: {status}")
                }
                Err(e) => {
                    println!("cargo:warning=test manifest resource compiler unavailable: {e}")
                }
            },
            None => {
                println!(
                    "cargo:warning=test manifest resource compiler not found (RC/rc.exe/vswhere); \
                     Tauri mock-runtime test binaries may fail before main"
                );
            }
        }
    }
}
