use opurom_platform::registry;

fn main() {
    println!("name | emitter | status | gpu | native_embed | multi_window");
    for contract in registry() {
        let caps = contract.capabilities;
        println!(
            "{} | {} | {} | {} | {} | {}",
            contract.name,
            contract.emitter,
            contract.status.as_str(),
            caps.gpu,
            caps.native_embed,
            caps.multi_window,
        );
    }
}
