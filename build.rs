fn main() {
    println!("cargo:rerun-if-changed=Assets/Radio.ico");
    println!("cargo:rerun-if-changed=Assets/DesktopRadio.manifest");
    let mut resources = winresource::WindowsResource::new();
    resources.set_icon("Assets/Radio.ico");
    resources.set_manifest_file("Assets/DesktopRadio.manifest");
    resources.set("ProductName", "Desktop Radio");
    resources.set("FileDescription", "Desktop Radio");
    resources.set("OriginalFilename", "DesktopRadio.exe");
    resources.compile().expect("compile Windows resources");
}
