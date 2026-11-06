//! Gives the Windows executable the application icon.

fn main() {
    println!("cargo:rerun-if-changed=assets/animatic.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/animatic.ico");
        resource
            .compile()
            .expect("embed the application icon into the Windows executable");
    }
}
