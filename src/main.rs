use opurom_platform::{host_platforms, registry};

fn main() {
    // Default output stays the 14 target rows; `--hosts` lists the Verified hosts.
    let hosts = std::env::args().any(|arg| arg.as_str() == "--hosts");
    let specs = if hosts { host_platforms() } else { registry() };
    println!("name | emitter | status | gpu | native_embed | multi_window");
    for spec in specs {
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
