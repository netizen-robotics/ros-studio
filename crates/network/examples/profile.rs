//! Development-only export of the exact generated localhost profile.
fn main() {
    let cyclone = std::env::args().nth(1).as_deref() == Some("cyclone");
    let config = studio_network::ConnectionConfig {
        domain: 187,
        middleware: studio_network::Middleware::from_cyclone(cyclone),
        selection: studio_network::Selection::Local,
        managed_environment_approved: true,
    };
    let profile =
        studio_network::profiles::Profile::prepare(&config).expect("prepare localhost profile");
    print!(
        "{}",
        std::fs::read_to_string(profile.path().unwrap()).expect("read generated profile")
    );
}
