use opurom_platform::{host_platforms, registry};

fn main() {
    // Default output stays the 14 target rows; `--hosts` lists the Verified hosts.
    let hosts = std::env::args().any(|arg| arg.as_str() == "--hosts");
    let contracts = if hosts { host_platforms() } else { registry() };
    println!("name | emitter | status | gpu | native_embed | multi_window");
    for contract in contracts {
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
