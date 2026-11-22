use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(glide_uninstaller)");
    let uninstall_only = env::var("CARGO_PKG_NAME").is_ok_and(|name| name == "glide-uninstall");
    if uninstall_only {
        println!("cargo:rustc-cfg=glide_uninstaller");
    }
    println!("cargo:rerun-if-env-changed=GLIDE_DOCK_PAYLOAD_ZIP");
    println!("cargo:rerun-if-env-changed=GLIDE_DOCK_UNINSTALL_EXE");
    println!("cargo:rerun-if-changed=../assets/brand/03-Windows-ICO/Glide-Dock-FullColor.ico");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let mut definition = String::new();
    for (variable, name) in [
        ("GLIDE_DOCK_PAYLOAD_ZIP", "EMBEDDED_PAYLOAD"),
        ("GLIDE_DOCK_UNINSTALL_EXE", "EMBEDDED_UNINSTALLER"),
    ] {
        if !uninstall_only && let Some(value) = env::var_os(variable) {
            let source = fs::canonicalize(value)
                .unwrap_or_else(|error| panic!("cannot open {variable}: {error}"));
            println!("cargo:rerun-if-changed={}", source.display());
            definition.push_str(&format!(
                "pub const {name}: &[u8] = include_bytes!({:?});\n",
                source
                    .to_str()
                    .expect("embedded artifact path must be Unicode")
            ));
        } else {
            definition.push_str(&format!("pub const {name}: &[u8] = &[];\n"));
        }
    }
    fs::write(out.join("embedded_payload.rs"), definition).expect("write embedded artifacts");

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(
                tauri_build::WindowsAttributes::new()
                    .window_icon_path("../assets/brand/03-Windows-ICO/Glide-Dock-FullColor.ico"),
            )
            .app_manifest(tauri_build::AppManifest::new().commands(if uninstall_only {
                &[
                    "setup_info",
                    "start_uninstall",
                    "operation_status",
                    "finish_operation",
                ]
            } else {
                &[
                    "setup_info",
                    "default_destination",
                    "check_destination",
                    "browse_destination",
                    "start_install",
                    "start_uninstall",
                    "operation_status",
                    "finish_operation",
                ]
            })),
    )
    .expect("configure the Tauri installer");
}
