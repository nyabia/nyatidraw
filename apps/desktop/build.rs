fn main() {
    #[cfg(windows)]
    if std::env::var_os("CARGO_FEATURE_GPUI").is_some() {
        println!("cargo:rerun-if-changed=assets/nyatidraw.rc");
        println!("cargo:rerun-if-changed=assets/nyatidraw.ico");
        embed_resource::compile_for(
            "assets/nyatidraw.rc",
            ["nyatidraw-gpui-desktop"],
            embed_resource::NONE,
        )
        .manifest_required()
        .expect("embed the Windows application icon");
    }
    if std::env::var_os("CARGO_FEATURE_GPUI").is_some()
        && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
    {
        // Rendering the full nested dock in a debug build exceeds the default
        // 1 MiB Win32 main stack. Windows commits the reserved pages on demand.
        let reserve = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            "/STACK:16777216"
        } else {
            "-Wl,--stack,16777216"
        };
        println!("cargo:rustc-link-arg-bin=nyatidraw-gpui-desktop={reserve}");
    }
}
