#[path = "../../build/skins.rs"]
mod skins;

fn main() {
    println!("Generating skins...");
    if let Err(e) = skins::generate() {
        eprintln!("Error generating skins: {e}");
        std::process::exit(1);
    }
    println!("Successfully generated ui/skins/generated_skins.slint");
}
