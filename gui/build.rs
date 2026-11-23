fn main() {
    let cfg = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", cfg).unwrap();
    // Put the icon into the Windows .exe (file explorer, taskbar pin). Never fails the build.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Slate Server");
        res.set("FileDescription", "Slate Server");
        let _ = res.compile();
    }
}
