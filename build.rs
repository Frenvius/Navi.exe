fn main() {
    // The UI has only keys; translations/<lang> holds their text (en, pt-BR), without contexts
    // so that src/i18n.rs reads the same catalogs.
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).unwrap();
    // Common Controls 6, as Explorer has: message boxes and the shell's own dialogs in our
    // process take the current look.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rerun-if-changed=ui/app.rc");
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
            "cargo:rustc-link-arg-bins=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
