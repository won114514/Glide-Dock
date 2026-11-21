use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=GLIDE_DOCK_PAYLOAD_ZIP");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let definition = if let Some(payload) = env::var_os("GLIDE_DOCK_PAYLOAD_ZIP") {
        let path = fs::canonicalize(payload).expect("cannot open GLIDE_DOCK_PAYLOAD_ZIP");
        println!("cargo:rerun-if-changed={}", path.display());
        format!(
            "pub const EMBEDDED_PAYLOAD: &[u8] = include_bytes!({:?});\n",
            path.to_str().expect("payload path must be Unicode")
        )
    } else {
        "pub const EMBEDDED_PAYLOAD: &[u8] = &[];\n".into()
    };
    fs::write(out.join("embedded_payload.rs"), definition).expect("write payload definition");

    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(
                tauri_build::WindowsAttributes::new()
                    .window_icon_path("../assets/brand/03-Windows-ICO/Glide-Dock-FullColor.ico"),
            )
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "setup_info",
                "default_destination",
                "check_destination",
                "browse_destination",
                "start_install",
                "start_uninstall",
                "operation_status",
                "finish_operation",
            ])),
    )
    .expect("configure the Tauri installer");
}
