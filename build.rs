fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("cosmic-dark".into());
    slint_build::compile_with_config("ui/capy-spell-player.slint", config)
        .expect("Slint build failed");
}
