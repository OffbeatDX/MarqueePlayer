// The window's markup lives with the app (crates/app/ui); it is compiled here.
fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("../app/ui/app.slint", config).expect("Slint build failed");
}
