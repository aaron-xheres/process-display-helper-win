fn main() {
    println!("cargo:rerun-if-changed=manifest.xml");
    println!("cargo:rerun-if-env-changed=PROCESS_DISPLAY_HELPER_SKIP_MANIFEST");

    if std::env::var_os("PROCESS_DISPLAY_HELPER_SKIP_MANIFEST").is_some()
        || std::env::var_os("CARGO_CFG_TEST").is_some()
    {
        return;
    }

    let mut resource = winres::WindowsResource::new();
    resource.set_manifest_file("manifest.xml");

    if let Err(error) = resource.compile() {
        panic!("failed to compile Windows resources: {error}");
    }
}
