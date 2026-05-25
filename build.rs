fn main() {
    let mut resource = winres::WindowsResource::new();
    resource.set_manifest_file("manifest.xml");

    if let Err(error) = resource.compile() {
        panic!("failed to compile Windows resources: {error}");
    }
}
