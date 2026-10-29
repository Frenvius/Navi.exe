fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rerun-if-changed=ui/app.rc");
        println!("cargo:rerun-if-changed=ui/app.manifest");
        println!("cargo:rerun-if-changed=ui/assets/explorer.ico");
        let kits = std::path::Path::new(
            &std::env::var("ProgramFiles(x86)")
                .unwrap_or_else(|_| r"C:\Program Files (x86)".into()),
        )
        .join("Windows Kits/10/bin");
        let mut versions: Vec<_> = std::fs::read_dir(kits)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join("x64/rc.exe").is_file())
            .collect();
        versions.sort();
        let compiler = std::env::var_os("RC")
            .map(std::path::PathBuf::from)
            .or_else(|| versions.pop().map(|p| p.join("x64/rc.exe")))
            .unwrap_or_else(|| "rc.exe".into());
        let output =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("explorer.res");
        let status = std::process::Command::new(compiler)
            .arg("/nologo")
            .arg("/c65001")
            .arg("/fo")
            .arg(&output)
            .arg("ui/app.rc")
            .status()
            .expect("Windows SDK resource compiler");
        assert!(
            status.success(),
            "app icon/version resource compilation failed"
        );
        println!("cargo:rustc-link-arg-bins={}", output.display());
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
                .join("ui/app.manifest")
                .display()
        );
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
