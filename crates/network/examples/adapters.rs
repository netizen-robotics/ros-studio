fn main() {
    match studio_network::adapters::enumerate() {
        Ok(adapters) => {
            for adapter in adapters {
                println!("{adapter:?}");
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
