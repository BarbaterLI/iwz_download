#[cfg(windows)]
fn main() {
    if std::path::Path::new("src/assets/icon.ico").exists() {
        let mut res = winres::WindowsResource::new();
        res.set_icon("src/assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to compile icon: {}, skipping resource compilation", e);
        }
    } else {
        println!("cargo:warning=Icon file not found at src/assets/icon.ico, skipping resource compilation");
    }
}

#[cfg(not(windows))]
fn main() {
    // Nothing to do on non-Windows platforms
}
