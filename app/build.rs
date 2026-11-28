fn main() {
    #[cfg(windows)]
    {
        let icon = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("assets")
            .join("brand")
            .join("03-Windows-ICO")
            .join("Glide-Dock-FullColor.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut resource = winres::WindowsResource::new();
        resource.set_icon(icon.to_string_lossy().as_ref());
        resource
            .compile()
            .expect("failed to embed the Glide-Dock Windows icon");
    }
}
