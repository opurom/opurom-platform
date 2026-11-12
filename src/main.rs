use opurom_platform::registry;

fn main() {
    println!("name | emitter | status | gpu | native_embed | multi_window");
    for spec in registry() {
        let caps = spec.capabilities;
        println!(
            "{} | {} | {} | {} | {} | {}",
            spec.name,
            spec.emitter,
            spec.status.as_str(),
            caps.gpu,
            caps.native_embed,
            caps.multi_window,
        );
    }
}
