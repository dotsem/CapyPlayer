#[path = "build/skins.rs"]
mod skins;

fn main() {
    println!("cargo:rerun-if-changed=ui/skins");
    skins::generate().expect("failed to generate skin registry and host");

    let config = slint_build::CompilerConfiguration::new().with_style("cosmic-dark".into());
    slint_build::compile_with_config("ui/capy-spell-player.slint", config)
        .expect("Slint build failed");
}
