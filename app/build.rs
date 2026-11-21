fn main() {
    println!("cargo:rerun-if-changed=ui/settings.slint");
    slint_build::compile("ui/settings.slint").expect("failed to compile Glide-Dock settings UI");

    #[cfg(windows)]
    {
        let icon = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("assets")
            .join("brand")
            .join("03-Windows-ICO")
            .join("Glide-Dock-FullColor.ico");
        let mut resource = winres::WindowsResource::new();
        resource.set_icon(icon.to_string_lossy().as_ref());
        resource
            .compile()
            .expect("failed to embed the Glide-Dock Windows icon");
    }
}
